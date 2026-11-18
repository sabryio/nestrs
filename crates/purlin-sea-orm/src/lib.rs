//! # purlin-sea-orm
//!
//! SeaORM, packaged the purlin way: a connection that is a provider with lifecycle hooks,
//! migrations behind a port, a health indicator, and transactions that follow the request.
//!
//! | Piece | Job |
//! |---|---|
//! | [`DatabaseConfig`] | how to connect (deserialisable, builder methods) |
//! | [`Database`] | the pool; connects, migrates on start, closes at shutdown, hands out [`Executor`]s |
//! | [`Executor`] | the pool *or* the request's transaction; implements SeaORM's `ConnectionTrait` |
//! | [`Transactional`] | interceptor: one transaction per request, commit on success, rollback on `4xx`/`5xx` |
//! | [`UnitOfWork`], [`Database::in_transaction`] | explicit transactions for jobs and scripts |
//! | [`Migrations`], [`SeaMigrator`], [`NoMigrations`] | the migration port and its adapters |
//! | [`DatabaseIndicator`] | readiness check named `database` |
//! | [`SeaOrmModule`] | wires all of it |
//!
//! ## Using it
//!
//! ```ignore
//! #[module(imports = [SeaOrmModule::with_migrator::<Migrator>(Provider::value(DatabaseConfig::new(url)))],
//!          providers = [SeaUserRepository], exports = [UserRepositoryPort])]
//! pub struct PersistenceModule;
//!
//! // A repository never knows whether a transaction is active:
//! async fn insert(&self, user: User) -> Result<(), RepositoryError> {
//!     let db = self.database.executor().await?;
//!     user::Entity::insert(row(user)).exec(&db).await?;
//!     Ok(())
//! }
//!
//! // Atomic writes on a route:
//! #[post("/")]
//! #[interceptor(Transactional)]
//! async fn create(&self, ..) -> .. { .. }
//! ```
//!
//! The crate enables no database driver; turn on the `sqlite`, `postgres` or `mysql`
//! feature you need.

#![forbid(unsafe_code)]

mod config;
mod database;
mod error;
mod executor;
mod health;
mod migrations;
mod module;
mod transaction;

pub use config::DatabaseConfig;
pub use database::{BoxFuture, Database};
pub use error::DbError;
pub use executor::Executor;
pub use health::DatabaseIndicator;
pub use migrations::{Migrations, MigrationsPort, NoMigrations, SeaMigrator};
pub use module::SeaOrmModule;
/// Re-exported so applications name SeaORM through one dependency path if they like.
pub use sea_orm;
pub use sea_orm_migration;
#[cfg(feature = "http")]
pub use transaction::Transactional;
pub use transaction::UnitOfWork;
