//! SeaORM adapters for the repository ports.
//!
//! The connection, transactions, migrations runner and health check come from
//! `nestrs-sea-orm`; this module holds only what is specific to the blog: entities, the
//! migrations themselves and the repositories.

mod entities;
mod migrations;
mod posts;
mod users;

pub use migrations::Migrator;
pub use posts::SeaPostRepository;
pub use users::SeaUserRepository;

use nestrs_sea_orm::{Database, Executor};

use crate::domain::RepositoryError;

/// Maps a driver error onto the port's error type, so no SeaORM type leaks inward.
fn unavailable(error: impl std::fmt::Display) -> RepositoryError {
    RepositoryError::Unavailable(error.to_string())
}

/// Where to run statements: the request's transaction when the route has one, the pool
/// otherwise. Repositories never need to know which.
async fn executor(database: &Database) -> Result<Executor, RepositoryError> {
    database.executor().await.map_err(unavailable)
}
