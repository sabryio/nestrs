use nestrs::prelude::*;
use sea_orm::{ConnectOptions, DatabaseConnection};
use sea_orm_migration::MigratorTrait;

use super::migrations::Migrator;
use super::unavailable;
use crate::domain::RepositoryError;
use crate::infrastructure::config::{AppConfig, Storage};

/// The database connection, created only when `storage = "database"`.
///
/// It is a provider with lifecycle hooks: connecting happens in the provider
/// function, `#[on_init]` applies pending migrations once the graph is up (unless
/// `migrate_on_start` is off, for deployments that migrate as a separate step), and
/// `#[on_destroy]` closes the pool at shutdown, after the repositories that use it
/// have been destroyed.
pub struct Database {
    connection: Option<DatabaseConnection>,
    migrate_on_start: bool,
}

impl Database {
    /// Connects according to `config`; in `Memory` mode there is nothing to connect to.
    pub async fn connect(config: &AppConfig) -> Result<Self, DiError> {
        if config.storage != Storage::Database {
            return Ok(Self {
                connection: None,
                migrate_on_start: false,
            });
        }
        let mut options = ConnectOptions::new(config.database_url.clone());
        options
            .sqlx_logging(false)
            .connect_timeout(std::time::Duration::from_secs(3));
        if config.database_url.contains(":memory:") {
            // Every SQLite in-memory connection is its own database: share one.
            options.max_connections(1).min_connections(1);
        }
        let connection = sea_orm::Database::connect(options)
            .await
            .map_err(|error| DiError::custom(format!("cannot connect to the database: {error}")))?;
        Ok(Self {
            connection: Some(connection),
            migrate_on_start: config.migrate_on_start,
        })
    }

    /// Applies every pending migration. A no-op without a database.
    pub async fn migrate(&self) -> Result<(), RepositoryError> {
        match &self.connection {
            Some(connection) => Migrator::up(connection, None).await.map_err(unavailable),
            None => Ok(()),
        }
    }

    /// Reverts the last `steps` applied migrations (newest first).
    pub async fn roll_back(&self, steps: u32) -> Result<(), RepositoryError> {
        let connection = self.connection()?;
        Migrator::down(connection, Some(steps)).await.map_err(unavailable)
    }

    /// Names of the migrations applied to this database, oldest first.
    pub async fn applied_migrations(&self) -> Result<Vec<String>, RepositoryError> {
        let connection = self.connection()?;
        let applied = Migrator::get_applied_migrations(connection)
            .await
            .map_err(unavailable)?;
        Ok(applied.iter().map(|migration| migration.name().to_owned()).collect())
    }

    /// Checks the connection is alive. Succeeds trivially without a database, where
    /// there is nothing to lose.
    pub async fn ping(&self) -> Result<(), RepositoryError> {
        match &self.connection {
            Some(connection) => connection.ping().await.map_err(unavailable),
            None => Ok(()),
        }
    }

    /// The live connection, or an error when the app runs without a database.
    pub(super) fn connection(&self) -> Result<&DatabaseConnection, RepositoryError> {
        self.connection
            .as_ref()
            .ok_or_else(|| RepositoryError::Unavailable("no database configured".to_owned()))
    }
}

#[lifecycle]
impl Database {
    /// Applies pending migrations once the graph is up.
    #[on_init]
    async fn apply_migrations(&self) -> Result<(), RepositoryError> {
        if self.migrate_on_start {
            self.migrate().await?;
        }
        Ok(())
    }

    /// Closes the pool at shutdown.
    #[on_destroy]
    async fn close(&self) -> Result<(), RepositoryError> {
        if let Some(connection) = &self.connection {
            connection.clone().close().await.map_err(unavailable)?;
        }
        Ok(())
    }
}
