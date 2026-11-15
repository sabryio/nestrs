//! One error type, so no driver type crosses the crate boundary.

use sea_orm::DbErr;

/// What can go wrong talking to the database.
///
/// Messages come from the driver and may contain host names; log them, do not show them to
/// API clients.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum DbError {
    /// The application runs without a database (see [`DatabaseConfig::enabled`](crate::DatabaseConfig)).
    #[error("no database is configured")]
    NotConfigured,
    /// The connection could not be established.
    #[error("cannot connect to the database: {0}")]
    Connect(String),
    /// A statement failed.
    #[error("database error: {0}")]
    Query(String),
    /// A migration failed.
    #[error("migration failed: {0}")]
    Migration(String),
    /// A transaction could not begin, commit or roll back.
    #[error("transaction failed: {0}")]
    Transaction(String),
}

impl From<DbErr> for DbError {
    fn from(error: DbErr) -> Self {
        Self::Query(error.to_string())
    }
}
