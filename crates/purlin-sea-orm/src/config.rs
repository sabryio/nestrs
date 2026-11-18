//! Connection settings.

use std::time::Duration;

use serde::Deserialize;

/// How to connect. Deserialisable, so it can sit inside the application's own configuration.
///
/// ```
/// use purlin_sea_orm::DatabaseConfig;
///
/// let config = DatabaseConfig::new("postgres://app@db/app").max_connections(20);
/// assert!(config.enabled && config.migrate_on_start);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct DatabaseConfig {
    /// Connection string (`sqlite::memory:`, `postgres://..`, `mysql://..`).
    pub url: String,
    /// `false` runs the application without a database: [`Database`](crate::Database) is
    /// [disconnected](crate::Database::disconnected), which suits in-memory modes and tests.
    pub enabled: bool,
    /// Upper bound of the pool; the driver's default when unset.
    pub max_connections: Option<u32>,
    /// Connections kept open when idle; the driver's default when unset.
    pub min_connections: Option<u32>,
    /// Seconds to wait for the first connection before startup fails.
    pub connect_timeout_secs: u64,
    /// Apply pending migrations when the application starts. Turn it off when migrations run
    /// as a separate deployment step.
    pub migrate_on_start: bool,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            url: "sqlite::memory:".to_owned(),
            enabled: true,
            max_connections: None,
            min_connections: None,
            connect_timeout_secs: 3,
            migrate_on_start: true,
        }
    }
}

impl DatabaseConfig {
    /// Settings for `url`, everything else at its default.
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            ..Self::default()
        }
    }

    /// Sets the pool's upper bound.
    pub fn max_connections(mut self, max: u32) -> Self {
        self.max_connections = Some(max);
        self
    }

    /// Sets how many connections the pool keeps open.
    pub fn min_connections(mut self, min: u32) -> Self {
        self.min_connections = Some(min);
        self
    }

    /// Sets how long startup waits for the first connection.
    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout_secs = timeout.as_secs().max(1);
        self
    }

    /// Chooses whether migrations run at start-up.
    pub fn migrate_on_start(mut self, yes: bool) -> Self {
        self.migrate_on_start = yes;
        self
    }

    /// Chooses whether a database is used at all.
    pub fn enabled(mut self, yes: bool) -> Self {
        self.enabled = yes;
        self
    }

    /// Every SQLite in-memory connection is its own database, so a pool would see several
    /// empty ones. Such a URL is pinned to one connection.
    pub(crate) fn is_private_memory_database(&self) -> bool {
        self.url.contains(":memory:")
    }
}
