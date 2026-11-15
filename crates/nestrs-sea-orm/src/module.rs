//! Packages everything as a module.

use std::sync::Arc;

use nestrs::prelude::*;
use sea_orm_migration::MigratorTrait;

use crate::health::DatabaseIndicator;
use crate::migrations::{Migrations, NoMigrations, SeaMigrator};
use crate::transaction::Transactional;
use crate::{Database, DatabaseConfig};

/// Import it to get a [`Database`] and the [`Transactional`] interceptor.
///
/// It exports both; the readiness check is contributed to the health set automatically.
/// You choose where the settings come from by passing a provider of [`DatabaseConfig`]:
///
/// ```ignore
/// // Fixed settings:
/// SeaOrmModule::with_migrator::<Migrator>(Provider::value(DatabaseConfig::new(url)))
///
/// // Derived from your own configuration provider:
/// SeaOrmModule::with_migrator::<Migrator>(Provider::factory::<DatabaseConfig>(
///     vec![Token::of::<AppConfig>()],
///     |container| Box::pin(async move {
///         let app = container.resolve::<AppConfig>()?;
///         Ok(DatabaseConfig::new(app.database_url.clone()))
///     }),
/// ))
/// ```
pub struct SeaOrmModule;

impl SeaOrmModule {
    /// Migrations come from the `sea-orm-migration` migrator `M`.
    pub fn with_migrator<M: MigratorTrait + 'static>(config: Provider) -> ModuleRef {
        Self::with_migrations(config, Arc::new(SeaMigrator::<M>::new()))
    }

    /// The schema is managed elsewhere.
    pub fn without_migrations(config: Provider) -> ModuleRef {
        Self::with_migrations(config, Arc::new(NoMigrations))
    }

    /// Your own [`Migrations`] implementation.
    ///
    /// # Panics
    ///
    /// If `config` does not provide a [`DatabaseConfig`]: that is a wiring mistake, and
    /// failing at module construction names it directly.
    pub fn with_migrations(config: Provider, migrations: Arc<dyn Migrations>) -> ModuleRef {
        assert_eq!(
            config.token(),
            Token::of::<DatabaseConfig>(),
            "SeaOrmModule needs a provider of `DatabaseConfig`"
        );
        let database = Provider::factory::<Database>(
            vec![Token::of::<DatabaseConfig>(), Token::of::<dyn Migrations>()],
            |container| {
                Box::pin(async move {
                    let config = container.resolve::<DatabaseConfig>()?;
                    let migrations = container.resolve::<dyn Migrations>()?;
                    Database::connect(&config, migrations)
                        .await
                        .map_err(|error| DiError::custom(error.to_string()))
                })
            },
        )
        .with_init::<Database>()
        .with_destroy::<Database>();

        let mut providers = vec![config, Provider::instance::<dyn Migrations>(migrations), database];
        providers.extend(DatabaseIndicator::register());
        providers.extend(Transactional::register());
        ModuleDef::new("SeaOrmModule")
            .providers(providers)
            .exports([Token::of::<Database>(), Token::of::<Transactional>()])
            .into_ref()
    }
}
