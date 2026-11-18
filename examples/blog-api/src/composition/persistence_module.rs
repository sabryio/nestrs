use purlin::prelude::*;
use purlin_outbox::{OutboxStorePort, SeaOutboxStore};
use purlin_sea_orm::{Database, DatabaseConfig, SeaOrmModule, Transactional};

use crate::domain::{PostAnnouncementsPort, PostRepositoryPort, UserRepositoryPort};
use crate::infrastructure::announcements::{NoAnnouncements, OutboxPostAnnouncements};
use crate::infrastructure::config::{AppConfig, Storage};
use crate::infrastructure::persistence::{
    InMemoryPostRepository, InMemoryUserRepository, Migrator, SeaPostRepository, SeaUserRepository,
};

/// Derives the connection settings from the application's own configuration. In
/// `Memory` mode the database is switched off, so the same module graph serves both modes.
fn database_config() -> Provider {
    Provider::factory::<DatabaseConfig>(vec![Token::of::<AppConfig>()], |container| {
        Box::pin(async move {
            let app = container.resolve::<AppConfig>()?;
            Ok(DatabaseConfig::new(app.database_url.clone())
                .enabled(app.storage == Storage::Database)
                .migrate_on_start(app.migrate_on_start))
        })
    })
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

/// Memory mode announces nothing; database mode records announcements in the outbox, inside
/// the request's transaction.
#[provider]
fn post_announcements(config: Arc<AppConfig>, database: Arc<Database>) -> PostAnnouncementsPort {
    match config.storage {
        Storage::Memory => Arc::new(NoAnnouncements),
        Storage::Database => Arc::new(OutboxPostAnnouncements::new(database)),
    }
}

/// Binds the repository ports to adapters chosen by `AppConfig::storage`.
///
/// Importers get the two ports and the [`Transactional`] interceptor; the concrete adapters
/// stay private. The connection, its migrations, its readiness check and its shutdown come
/// from `purlin_sea_orm::SeaOrmModule`, so nothing here knows how a pool is managed.
#[module(
    imports = [SeaOrmModule::with_migrator::<Migrator>(database_config())],
    providers = [user_repository, post_repository, post_announcements, SeaOutboxStore],
    exports = [UserRepositoryPort, PostRepositoryPort, PostAnnouncementsPort, OutboxStorePort, Transactional],
)]
pub struct PersistenceModule;
