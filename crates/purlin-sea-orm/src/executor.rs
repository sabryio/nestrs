//! The handle repositories run statements on.

use std::sync::Arc;

use purlin::async_trait;
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbBackend, DbErr, ExecResult, QueryResult, Statement,
};

/// Either the pool or the transaction the current request opened.
///
/// It implements SeaORM's [`ConnectionTrait`], so `Entity::find().one(&executor)` and every
/// other query builder works unchanged. A repository asks the [`Database`](crate::Database)
/// for one per operation and never needs to know whether a transaction is active; that is
/// what lets [`Transactional`](crate::Transactional) be added to a route without touching
/// repository code.
#[derive(Clone)]
pub struct Executor {
    target: Target,
}

#[derive(Clone)]
enum Target {
    Pool(DatabaseConnection),
    Transaction(Arc<DatabaseTransaction>),
}

impl Executor {
    pub(crate) fn pool(connection: DatabaseConnection) -> Self {
        Self {
            target: Target::Pool(connection),
        }
    }

    pub(crate) fn transaction(transaction: Arc<DatabaseTransaction>) -> Self {
        Self {
            target: Target::Transaction(transaction),
        }
    }

    /// Whether statements run inside a transaction.
    pub fn in_transaction(&self) -> bool {
        matches!(self.target, Target::Transaction(_))
    }
}

impl std::fmt::Debug for Executor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Executor")
            .field("in_transaction", &self.in_transaction())
            .finish()
    }
}

#[async_trait]
impl ConnectionTrait for Executor {
    fn get_database_backend(&self) -> DbBackend {
        match &self.target {
            Target::Pool(connection) => connection.get_database_backend(),
            Target::Transaction(transaction) => transaction.get_database_backend(),
        }
    }

    async fn execute(&self, statement: Statement) -> Result<ExecResult, DbErr> {
        match &self.target {
            Target::Pool(connection) => connection.execute(statement).await,
            Target::Transaction(transaction) => transaction.execute(statement).await,
        }
    }

    async fn execute_unprepared(&self, sql: &str) -> Result<ExecResult, DbErr> {
        match &self.target {
            Target::Pool(connection) => connection.execute_unprepared(sql).await,
            Target::Transaction(transaction) => transaction.execute_unprepared(sql).await,
        }
    }

    async fn query_one(&self, statement: Statement) -> Result<Option<QueryResult>, DbErr> {
        match &self.target {
            Target::Pool(connection) => connection.query_one(statement).await,
            Target::Transaction(transaction) => transaction.query_one(statement).await,
        }
    }

    async fn query_all(&self, statement: Statement) -> Result<Vec<QueryResult>, DbErr> {
        match &self.target {
            Target::Pool(connection) => connection.query_all(statement).await,
            Target::Transaction(transaction) => transaction.query_all(statement).await,
        }
    }
}
