//! SeaORM adapters for the repository ports.

mod database;
mod entities;
mod health;
mod migrations;
mod posts;
mod users;

pub use database::Database;
pub use health::DatabaseIndicator;
pub use migrations::Migrator;
pub use posts::SeaPostRepository;
pub use users::SeaUserRepository;

use crate::domain::RepositoryError;

/// Maps a driver error onto the port's error type, so no SeaORM type leaks inward.
fn unavailable(error: sea_orm::DbErr) -> RepositoryError {
    RepositoryError::Unavailable(error.to_string())
}
