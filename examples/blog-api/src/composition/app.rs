use nestrs::prelude::*;
use nestrs_kit::KitModule;

use super::{
    MessagingModule, PersistenceModule, PostsModule, SeedModule, SharedStateModule, UsersModule, outbox_relay,
};
use crate::infrastructure::config::AppConfig;
use nestrs::config::{ConfigModule, ConfigSource};
use nestrs_health::HealthModule;

/// Root module. To move to a real database, set `BLOG_STORAGE=database`; the
/// provider functions in [`PersistenceModule`] pick the adapters and nothing else
/// changes. Tests swap single providers with
/// `App::builder(AppModule).override_provider(..)` instead.
#[module(
    imports     = [
        ConfigModule::for_root::<AppConfig>(ConfigSource::new().file("blog-api.toml").env_prefix("BLOG")),
        HttpModule::module(),
        KitModule,
        HealthModule,
        PersistenceModule,
        MessagingModule,
        outbox_relay(),
        SharedStateModule,
        UsersModule,
        PostsModule,
        SeedModule,
    ],
)]
pub struct AppModule;
