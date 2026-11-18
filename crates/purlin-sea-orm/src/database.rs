//! The managed connection.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use purlin::prelude::*;
use sea_orm::{ConnectOptions, DatabaseConnection, TransactionTrait};

use crate::migrations::{MigrationsPort, NoMigrations};
use crate::transaction::{Slot, UnitOfWork};
use crate::{DatabaseConfig, DbError, Executor};

/// A future that borrows from its arguments, as [`Database::in_transaction`] needs.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// The database connection pool plus what is needed to manage it.
///
/// It is a provider with lifecycle hooks: [`SeaOrmModule`](crate::SeaOrmModule) builds it,
/// applies pending migrations once the graph is up (unless
/// [`migrate_on_start`](DatabaseConfig::migrate_on_start) is off) and closes the pool at
/// shutdown, after the repositories that use it are gone.
///
/// Repositories call [`executor`](Self::executor) per operation.
pub struct Database {
    id: u64,
    connection: Option<DatabaseConnection>,
    migrations: MigrationsPort,
    migrate_on_start: bool,
}

impl Database {
    /// Connects according to `config`. With `enabled = false` nothing is connected.
    pub async fn connect(config: &DatabaseConfig, migrations: MigrationsPort) -> Result<Self, DbError> {
        if !config.enabled {
            return Ok(Self::disconnected());
        }
        let mut options = ConnectOptions::new(config.url.clone());
        options
            .sqlx_logging(false)
            .connect_timeout(Duration::from_secs(config.connect_timeout_secs));
        if let Some(max) = config.max_connections {
            options.max_connections(max);
        }
        if let Some(min) = config.min_connections {
            options.min_connections(min);
        }
        if config.is_private_memory_database() {
            options.max_connections(1).min_connections(1);
        }
        let connection = sea_orm::Database::connect(options)
            .await
            .map_err(|error| DbError::Connect(error.to_string()))?;
        Ok(Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            connection: Some(connection),
            migrations,
            migrate_on_start: config.migrate_on_start,
        })
    }

    /// A database that is not there: health checks and migrations succeed trivially,
    /// [`executor`](Self::executor) returns [`DbError::NotConfigured`]. It lets a module
    /// graph keep one shape whether or not a database is in use.
    pub fn disconnected() -> Self {
        Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            connection: None,
            migrations: std::sync::Arc::new(NoMigrations),
            migrate_on_start: false,
        }
    }

    /// Whether a database is connected.
    pub fn is_connected(&self) -> bool {
        self.connection.is_some()
    }

    #[cfg(feature = "http")]
    pub(crate) fn id(&self) -> u64 {
        self.id
    }

    /// The pool itself, for the rare caller that needs a raw SeaORM handle (it bypasses the
    /// request's transaction).
    pub fn connection(&self) -> Result<&DatabaseConnection, DbError> {
        self.connection.as_ref().ok_or(DbError::NotConfigured)
    }

    /// Where to run statements: the current request's transaction when [`Transactional`]
    /// wraps it, the pool otherwise.
    ///
    /// [`Transactional`]: crate::Transactional
    pub async fn executor(&self) -> Result<Executor, DbError> {
        let connection = self.connection()?;
        match Slot::current(self.id) {
            Some(slot) => slot.executor(connection).await,
            None => Ok(Executor::pool(connection.clone())),
        }
    }

    /// Starts an explicit transaction.
    pub async fn begin(&self) -> Result<UnitOfWork, DbError> {
        let transaction = self
            .connection()?
            .begin()
            .await
            .map_err(|error| DbError::Transaction(error.to_string()))?;
        Ok(UnitOfWork::new(transaction))
    }

    /// Runs `work` in a transaction: commit on `Ok`, roll back on `Err`.
    ///
    /// Inside a request wrapped by [`Transactional`](crate::Transactional) it joins that
    /// transaction instead of starting another, and the request decides the outcome.
    ///
    /// ```ignore
    /// database
    ///     .in_transaction(|tx| Box::pin(async move {
    ///         orders::insert(tx, &order).await?;
    ///         outbox::enqueue(tx, &event).await?;
    ///         Ok::<_, MyError>(())
    ///     }))
    ///     .await?;
    /// ```
    pub async fn in_transaction<T, E, F>(&self, work: F) -> Result<T, E>
    where
        F: for<'a> FnOnce(&'a Executor) -> BoxFuture<'a, Result<T, E>>,
        E: From<DbError>,
    {
        if Slot::current(self.id).is_some() {
            let executor = self.executor().await?;
            return work(&executor).await;
        }
        let unit = self.begin().await?;
        let executor = unit.executor();
        let result = work(&executor).await;
        drop(executor);
        match result {
            Ok(value) => {
                unit.commit().await?;
                Ok(value)
            }
            Err(error) => {
                if let Err(rollback) = unit.rollback().await {
                    tracing::warn!(%rollback, "rollback failed");
                }
                Err(error)
            }
        }
    }

    /// Applies every pending migration. A no-op without a database.
    pub async fn migrate(&self) -> Result<(), DbError> {
        match &self.connection {
            Some(connection) => self.migrations.up(connection).await,
            None => Ok(()),
        }
    }

    /// Reverts the last `steps` applied migrations (newest first).
    pub async fn roll_back(&self, steps: u32) -> Result<(), DbError> {
        self.migrations.down(self.connection()?, steps).await
    }

    /// Names of the migrations applied to this database, oldest first.
    pub async fn applied_migrations(&self) -> Result<Vec<String>, DbError> {
        self.migrations.applied(self.connection()?).await
    }

    /// Checks the connection is alive. Succeeds trivially without a database, where there is
    /// nothing to lose.
    pub async fn ping(&self) -> Result<(), DbError> {
        match &self.connection {
            Some(connection) => connection.ping().await.map_err(DbError::from),
            None => Ok(()),
        }
    }
}

impl OnModuleInit for Database {
    async fn on_module_init(&self) -> Result<(), DiError> {
        if self.migrate_on_start {
            self.migrate()
                .await
                .map_err(|error| DiError::custom(error.to_string()))?;
        }
        Ok(())
    }
}

impl OnModuleDestroy for Database {
    async fn on_module_destroy(&self) -> Result<(), DiError> {
        if let Some(connection) = &self.connection {
            connection
                .clone()
                .close()
                .await
                .map_err(|error| DiError::custom(error.to_string()))?;
        }
        Ok(())
    }
}
