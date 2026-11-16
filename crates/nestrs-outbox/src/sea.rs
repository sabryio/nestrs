//! The SeaORM side: write a message inside a transaction, and a store the relay can claim from.
//!
//! The SQL is built with `sea-query`, so one implementation serves SQLite, Postgres and MySQL.
//! Claiming does not rely on `SKIP LOCKED`: it picks candidates, then updates them guarded by
//! "not claimed (or claim expired)", so two relays can never both win the same row.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nestrs::prelude::*;
use nestrs_sea_orm::sea_orm::{ConnectionTrait, QueryResult, StatementBuilder};
use nestrs_sea_orm::sea_orm_migration::prelude::Query;
use nestrs_sea_orm::sea_orm_migration::prelude::*;
use nestrs_sea_orm::{Database, DbError, Executor};
use serde::Serialize;

use crate::{NewMessage, OutboxError, OutboxMessage, OutboxStore, OutboxStorePort, StoreError};

#[derive(DeriveIden)]
enum Messages {
    #[sea_orm(iden = "nestrs_outbox")]
    Table,
    Seq,
    Id,
    Topic,
    OrderingKey,
    Payload,
    Attempts,
    CreatedAt,
    AvailableAt,
    ClaimedUntil,
    ClaimToken,
    Status,
    LastError,
}

const PENDING: &str = "pending";
const DEAD: &str = "dead";

/// Creates the outbox table. Add it to your migrator:
///
/// ```ignore
/// fn migrations() -> Vec<Box<dyn MigrationTrait>> {
///     vec![Box::new(m20261110_000001_create_users::Migration), Box::new(nestrs_outbox::CreateOutbox)]
/// }
/// ```
pub struct CreateOutbox;

impl MigrationName for CreateOutbox {
    fn name(&self) -> &str {
        "m20261115_000001_create_nestrs_outbox"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for CreateOutbox {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Messages::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Messages::Seq)
                            .big_integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Messages::Id).string_len(64).not_null().unique_key())
                    .col(ColumnDef::new(Messages::Topic).string_len(255).not_null())
                    .col(ColumnDef::new(Messages::OrderingKey).string_len(255).null())
                    .col(ColumnDef::new(Messages::Payload).text().not_null())
                    .col(ColumnDef::new(Messages::Attempts).integer().not_null().default(0))
                    .col(ColumnDef::new(Messages::CreatedAt).big_integer().not_null())
                    .col(
                        ColumnDef::new(Messages::AvailableAt)
                            .big_integer()
                            .not_null()
                            .default(0),
                    )
                    .col(ColumnDef::new(Messages::ClaimedUntil).big_integer().null())
                    .col(ColumnDef::new(Messages::ClaimToken).string_len(64).null())
                    .col(ColumnDef::new(Messages::Status).string_len(16).not_null())
                    .col(ColumnDef::new(Messages::LastError).text().null())
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_nestrs_outbox_due")
                    .table(Messages::Table)
                    .col(Messages::Status)
                    .col(Messages::AvailableAt)
                    .if_not_exists()
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_nestrs_outbox_key")
                    .table(Messages::Table)
                    .col(Messages::OrderingKey)
                    .col(Messages::Seq)
                    .if_not_exists()
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Messages::Table).if_exists().to_owned())
            .await
    }
}

/// Records messages. Stateless: construct it anywhere (`Outbox`).
///
/// Pass the [`Executor`] you already use for your own writes. Inside a request wrapped by
/// `Transactional` that is the request's transaction, so the message commits or rolls back
/// together with your data; that is the whole point.
#[derive(Debug, Default, Clone, Copy)]
pub struct Outbox;

impl Outbox {
    /// Records `message` using `db`; returns the message id.
    pub async fn record(&self, db: &Executor, message: NewMessage) -> Result<String, OutboxError> {
        let id = uuid::Uuid::new_v4().to_string();
        let statement = Query::insert()
            .into_table(Messages::Table)
            .columns([
                Messages::Id,
                Messages::Topic,
                Messages::OrderingKey,
                Messages::Payload,
                Messages::Attempts,
                Messages::CreatedAt,
                Messages::AvailableAt,
                Messages::Status,
            ])
            .values_panic([
                id.clone().into(),
                message.topic.into(),
                message.key.into(),
                message.payload.into(),
                0i32.into(),
                now_ms().into(),
                0i64.into(),
                PENDING.into(),
            ])
            .to_owned();
        run(db, &statement)
            .await
            .map_err(|error| OutboxError::Store(error.to_string()))?;
        Ok(id)
    }

    /// Serialises `payload` and records it.
    pub async fn record_json<T: Serialize>(
        &self,
        db: &Executor,
        topic: &str,
        key: Option<&str>,
        payload: &T,
    ) -> Result<String, OutboxError> {
        self.record(db, NewMessage::json(topic, key, payload)?).await
    }
}

fn now_ms() -> i64 {
    let elapsed = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    elapsed.as_millis().try_into().unwrap_or(i64::MAX)
}

async fn run(db: &Executor, statement: &impl StatementBuilder) -> Result<u64, DbErr> {
    let built = db.get_database_backend().build(statement);
    db.execute(built).await.map(|result| result.rows_affected())
}

async fn fetch(db: &Executor, statement: &impl StatementBuilder) -> Result<Vec<QueryResult>, DbErr> {
    let built = db.get_database_backend().build(statement);
    db.query_all(built).await
}

fn store_error(error: impl std::fmt::Display) -> StoreError {
    StoreError(error.to_string())
}

fn message_from(row: &QueryResult, claim: &str) -> Result<OutboxMessage, DbErr> {
    Ok(OutboxMessage {
        id: row.try_get("", "id")?,
        topic: row.try_get("", "topic")?,
        key: row.try_get("", "ordering_key")?,
        payload: row.try_get("", "payload")?,
        attempts: row.try_get::<i32>("", "attempts")?.max(0).unsigned_abs(),
        created_at_ms: row.try_get("", "created_at")?,
        claim: claim.to_owned(),
    })
}

/// The outbox table through SeaORM.
#[injectable(provides = OutboxStorePort)]
pub struct SeaOutboxStore {
    database: Arc<Database>,
}

impl SeaOutboxStore {
    /// A store over `database`.
    pub fn new(database: Arc<Database>) -> Self {
        Self { database }
    }

    async fn pool(&self) -> Result<Executor, StoreError> {
        self.database.executor().await.map_err(store_error)
    }

    async fn count(&self, status: &str) -> Result<usize, StoreError> {
        let db = self.pool().await?;
        let statement = Query::select()
            .expr_as(Expr::cust("COUNT(*)"), Alias::new("n"))
            .from(Messages::Table)
            .and_where(Expr::col(Messages::Status).eq(status))
            .to_owned();
        let rows = fetch(&db, &statement).await.map_err(store_error)?;
        let n: i64 = rows
            .first()
            .ok_or_else(|| StoreError("COUNT returned no row".to_owned()))?
            .try_get("", "n")
            .map_err(store_error)?;
        Ok(usize::try_from(n).unwrap_or(0))
    }

    /// Runs `update` against the row that `message` still claims.
    async fn update_claimed(&self, message: &OutboxMessage, statement: UpdateStatement) -> Result<bool, StoreError> {
        let db = self.pool().await?;
        let mut statement = statement;
        statement
            .and_where(Expr::col(Messages::Id).eq(message.id.as_str()))
            .and_where(Expr::col(Messages::ClaimToken).eq(message.claim.as_str()));
        Ok(run(&db, &statement).await.map_err(store_error)? > 0)
    }
}

/// Candidates, then a guarded claim, then a read of what this claim won; one transaction.
async fn claim_in(db: &Executor, now: i64, limit: usize, lease: Duration) -> Result<Vec<OutboxMessage>, DbErr> {
    let older_pending_with_same_key = Query::select()
        .expr(Expr::val(1))
        .from_as(Messages::Table, Alias::new("older"))
        .and_where(
            Expr::col((Alias::new("older"), Messages::OrderingKey)).equals((Alias::new("m"), Messages::OrderingKey)),
        )
        .and_where(Expr::col((Alias::new("older"), Messages::Seq)).lt(Expr::col((Alias::new("m"), Messages::Seq))))
        .and_where(Expr::col((Alias::new("older"), Messages::Status)).eq(PENDING))
        .to_owned();
    let candidates = Query::select()
        .column((Alias::new("m"), Messages::Id))
        .from_as(Messages::Table, Alias::new("m"))
        .and_where(Expr::col((Alias::new("m"), Messages::Status)).eq(PENDING))
        .and_where(Expr::col((Alias::new("m"), Messages::AvailableAt)).lte(now))
        .and_where(
            Expr::col((Alias::new("m"), Messages::ClaimedUntil))
                .is_null()
                .or(Expr::col((Alias::new("m"), Messages::ClaimedUntil)).lte(now)),
        )
        .and_where(Expr::exists(older_pending_with_same_key).not())
        .order_by((Alias::new("m"), Messages::Seq), Order::Asc)
        .limit(limit as u64)
        .to_owned();
    let ids: Vec<String> = fetch(db, &candidates)
        .await?
        .iter()
        .map(|row| row.try_get("", "id"))
        .collect::<Result<_, _>>()?;
    if ids.is_empty() {
        return Ok(Vec::new());
    }

    // At most one candidate per key: two messages of one key can both be candidates only if
    // neither has an older pending sibling, which is impossible (the younger one has one).
    let token = uuid::Uuid::new_v4().to_string();
    let lease_ms: i64 = lease.as_millis().try_into().unwrap_or(i64::MAX);
    let claim = Query::update()
        .table(Messages::Table)
        .values([
            (Messages::ClaimedUntil, now.saturating_add(lease_ms).into()),
            (Messages::ClaimToken, token.clone().into()),
        ])
        .and_where(Expr::col(Messages::Id).is_in(ids))
        .and_where(Expr::col(Messages::Status).eq(PENDING))
        // The guard that makes concurrent relays safe: a row someone else just claimed no
        // longer matches, so it is skipped instead of claimed twice.
        .and_where(
            Expr::col(Messages::ClaimedUntil)
                .is_null()
                .or(Expr::col(Messages::ClaimedUntil).lte(now)),
        )
        .to_owned();
    if run(db, &claim).await? == 0 {
        return Ok(Vec::new());
    }

    let won = Query::select()
        .columns([
            Messages::Id,
            Messages::Topic,
            Messages::OrderingKey,
            Messages::Payload,
            Messages::Attempts,
            Messages::CreatedAt,
        ])
        .from(Messages::Table)
        .and_where(Expr::col(Messages::ClaimToken).eq(token.as_str()))
        .order_by(Messages::Seq, Order::Asc)
        .to_owned();
    fetch(db, &won)
        .await?
        .iter()
        .map(|row| message_from(row, &token))
        .collect()
}

#[async_trait]
impl OutboxStore for SeaOutboxStore {
    async fn claim(&self, now_ms: i64, limit: usize, lease: Duration) -> Result<Vec<OutboxMessage>, StoreError> {
        let unit = self.database.begin().await.map_err(store_error)?;
        let db = unit.executor();
        let claimed = claim_in(&db, now_ms, limit, lease).await;
        drop(db);
        match claimed {
            Ok(messages) => {
                unit.commit().await.map_err(store_error)?;
                Ok(messages)
            }
            Err(error) => {
                if let Err(rollback) = unit.rollback().await {
                    tracing::warn!(%rollback, "outbox: rollback after a failed claim also failed");
                }
                Err(store_error(error))
            }
        }
    }

    async fn complete(&self, message: &OutboxMessage) -> Result<bool, StoreError> {
        let db = self.pool().await?;
        let statement = Query::delete()
            .from_table(Messages::Table)
            .and_where(Expr::col(Messages::Id).eq(message.id.as_str()))
            .and_where(Expr::col(Messages::ClaimToken).eq(message.claim.as_str()))
            .to_owned();
        Ok(run(&db, &statement).await.map_err(store_error)? > 0)
    }

    async fn release(&self, message: &OutboxMessage, error: &str, retry_at_ms: i64) -> Result<bool, StoreError> {
        let statement = Query::update()
            .table(Messages::Table)
            .values([
                (Messages::Attempts, Expr::col(Messages::Attempts).add(1)),
                (Messages::AvailableAt, retry_at_ms.into()),
                (Messages::ClaimedUntil, Expr::value(Option::<i64>::None)),
                (Messages::ClaimToken, Expr::value(Option::<String>::None)),
                (Messages::LastError, error.into()),
            ])
            .to_owned();
        self.update_claimed(message, statement).await
    }

    async fn bury(&self, message: &OutboxMessage, error: &str) -> Result<bool, StoreError> {
        let statement = Query::update()
            .table(Messages::Table)
            .values([
                (Messages::Attempts, Expr::col(Messages::Attempts).add(1)),
                (Messages::Status, DEAD.into()),
                (Messages::ClaimedUntil, Expr::value(Option::<i64>::None)),
                (Messages::ClaimToken, Expr::value(Option::<String>::None)),
                (Messages::LastError, error.into()),
            ])
            .to_owned();
        self.update_claimed(message, statement).await
    }

    async fn pending_len(&self) -> Result<usize, StoreError> {
        self.count(PENDING).await
    }

    async fn dead_len(&self) -> Result<usize, StoreError> {
        self.count(DEAD).await
    }

    async fn requeue_dead(&self, id: &str) -> Result<bool, StoreError> {
        let db = self.pool().await?;
        let statement = Query::update()
            .table(Messages::Table)
            .values([
                (Messages::Status, PENDING.into()),
                (Messages::Attempts, 0i32.into()),
                (Messages::AvailableAt, 0i64.into()),
            ])
            .and_where(Expr::col(Messages::Id).eq(id))
            .and_where(Expr::col(Messages::Status).eq(DEAD))
            .to_owned();
        Ok(run(&db, &statement).await.map_err(store_error)? > 0)
    }
}

impl From<DbError> for OutboxError {
    fn from(error: DbError) -> Self {
        Self::Store(error.to_string())
    }
}
