//! Typed application configuration, loaded by `nestrs::config::ConfigModule`.

use serde::Deserialize;
use validator::Validate;

/// Which persistence adapters the application uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Storage {
    /// Process memory; data is lost on restart. Good for demos and tests.
    Memory,
    /// A SQL database through SeaORM (`database_url`).
    Database,
}

/// Settings with development-friendly defaults.
///
/// Layers, lowest priority first: these defaults, `blog-api.toml` (optional), then
/// environment variables prefixed `BLOG_` (`BLOG_API_TOKEN`, `BLOG_BIND_ADDR`,
/// `BLOG_STORAGE=database`, `BLOG_DATABASE_URL=postgres://..`, `BLOG_MIGRATE_ON_START=false`,
/// `BLOG_SEED_DEMO_DATA=true`, `BLOG_REDIS_URL=redis://host:6379/`).
#[derive(Debug, Clone, Deserialize, Validate)]
#[serde(default)]
pub struct AppConfig {
    /// Address the HTTP server binds to.
    #[validate(length(min = 1, message = "must not be empty"))]
    pub bind_addr: String,
    /// Bearer token of the `admin` (everything allowed); see
    /// [`StaticTokenVerifier`](super::auth::StaticTokenVerifier).
    #[validate(length(min = 1, message = "must not be empty"))]
    pub api_token: String,
    /// Bearer token of an `author`: may publish posts, nothing else.
    #[validate(length(min = 1, message = "must not be empty"))]
    pub author_token: String,
    /// Whether to insert a demo author and post at start-up.
    pub seed_demo_data: bool,
    /// Which persistence adapters to use.
    pub storage: Storage,
    /// Connection string used when `storage = "database"` (SQLite or Postgres).
    #[validate(length(min = 1, message = "must not be empty"))]
    pub database_url: String,
    /// Apply pending migrations at start-up (default). Turn it off when migrations
    /// run as a separate deployment step (`BLOG_MIGRATE_ON_START=false`).
    pub migrate_on_start: bool,
    /// Shared state for several instances (idempotency keys). Unset keeps it in process
    /// memory, which is exact for one instance only.
    pub redis_url: Option<String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            bind_addr: "127.0.0.1:3000".to_owned(),
            api_token: "dev-token".to_owned(),
            author_token: "author-token".to_owned(),
            seed_demo_data: false,
            storage: Storage::Memory,
            database_url: "sqlite::memory:".to_owned(),
            migrate_on_start: true,
            redis_url: None,
        }
    }
}
