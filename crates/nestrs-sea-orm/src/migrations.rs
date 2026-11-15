//! Migrations behind a port, so the connection does not need to know the migrator's type.

use std::marker::PhantomData;

use nestrs::prelude::*;
use sea_orm::DatabaseConnection;
use sea_orm_migration::MigratorTrait;

use crate::DbError;

/// Applies, reverts and lists schema migrations.
///
/// [`SeaMigrator`] adapts a `sea-orm-migration` migrator; [`NoMigrations`] is the empty one.
/// Tests can implement it to observe or fail migrations.
#[port]
pub trait Migrations {
    /// Applies every pending migration.
    async fn up(&self, connection: &DatabaseConnection) -> Result<(), DbError>;

    /// Reverts the last `steps` applied migrations, newest first.
    async fn down(&self, connection: &DatabaseConnection, steps: u32) -> Result<(), DbError>;

    /// Names of the applied migrations, oldest first.
    async fn applied(&self, connection: &DatabaseConnection) -> Result<Vec<String>, DbError>;
}

/// Adapter from a `sea-orm-migration` [`MigratorTrait`] to [`Migrations`].
///
/// ```ignore
/// let migrations = SeaMigrator::<crate::migrations::Migrator>::new();
/// ```
pub struct SeaMigrator<M>(PhantomData<fn() -> M>);

impl<M> SeaMigrator<M> {
    /// Creates the adapter.
    pub fn new() -> Self {
        Self(PhantomData)
    }
}

impl<M> Default for SeaMigrator<M> {
    fn default() -> Self {
        Self::new()
    }
}

fn failed(error: sea_orm::DbErr) -> DbError {
    DbError::Migration(error.to_string())
}

#[async_trait]
impl<M> Migrations for SeaMigrator<M>
where
    M: MigratorTrait + 'static,
{
    async fn up(&self, connection: &DatabaseConnection) -> Result<(), DbError> {
        M::up(connection, None).await.map_err(failed)
    }

    async fn down(&self, connection: &DatabaseConnection, steps: u32) -> Result<(), DbError> {
        M::down(connection, Some(steps)).await.map_err(failed)
    }

    async fn applied(&self, connection: &DatabaseConnection) -> Result<Vec<String>, DbError> {
        let applied = M::get_applied_migrations(connection).await.map_err(failed)?;
        Ok(applied.iter().map(|migration| migration.name().to_owned()).collect())
    }
}

/// For applications whose schema is managed elsewhere.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoMigrations;

#[async_trait]
impl Migrations for NoMigrations {
    async fn up(&self, _connection: &DatabaseConnection) -> Result<(), DbError> {
        Ok(())
    }

    async fn down(&self, _connection: &DatabaseConnection, _steps: u32) -> Result<(), DbError> {
        Ok(())
    }

    async fn applied(&self, _connection: &DatabaseConnection) -> Result<Vec<String>, DbError> {
        Ok(Vec::new())
    }
}
