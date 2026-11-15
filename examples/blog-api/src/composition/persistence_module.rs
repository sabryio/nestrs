use nestrs::prelude::*;

use crate::domain::{PostRepositoryPort, UserRepositoryPort};
use crate::infrastructure::config::{AppConfig, Storage};
use crate::infrastructure::persistence::{
    Database, DatabaseIndicator, InMemoryPostRepository, InMemoryUserRepository, SeaPostRepository, SeaUserRepository,
};

/// Connects the database (lifecycle hooks come with `Database`).
#[provider]
async fn database(config: Arc<AppConfig>) -> Result<Database, DiError> {
    Database::connect(&config).await
}

/// Picks the user repository adapter from `AppConfig::storage`.
#[provider]
fn user_repository(config: Arc<AppConfig>, database: Arc<Database>) -> UserRepositoryPort {
    match config.storage {
        Storage::Memory => Arc::new(InMemoryUserRepository::default()),
        Storage::Database => Arc::new(SeaUserRepository::new(database)),
    }
}

/// Picks the post repository adapter from `AppConfig::storage`.
#[provider]
fn post_repository(config: Arc<AppConfig>, database: Arc<Database>) -> PostRepositoryPort {
    match config.storage {
        Storage::Memory => Arc::new(InMemoryPostRepository::default()),
        Storage::Database => Arc::new(SeaPostRepository::new(database)),
    }
}

/// Binds the repository ports to adapters chosen by `AppConfig::storage`, and
/// exports only the ports.
///
/// Importers get `UserRepositoryPort` and `PostRepositoryPort`; the concrete
/// adapters stay private. The choice is made when the providers are built
/// (configuration is itself a provider), so the same `AppModule` serves every
/// storage mode. `Database` carries its own lifecycle hooks: it creates tables on
/// init and closes the pool on shutdown, after the repositories are gone.
#[module(
    providers = [database, user_repository, post_repository, DatabaseIndicator],
    exports = [UserRepositoryPort, PostRepositoryPort],
)]
pub struct PersistenceModule;
