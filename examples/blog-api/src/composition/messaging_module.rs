use nestrs::prelude::*;
use nestrs_jobs::JobsModule;
use nestrs_outbox::{OutboxConfig, OutboxModule, OutboxPublisherPort, QueuePublisher};

use super::PersistenceModule;
use crate::infrastructure::announcements::{Announced, PostPublishedHandler};
use crate::infrastructure::config::{AppConfig, Storage};

/// Where announcements go: a queue, and the consumer of that queue. The queue is in-process
/// here; swapping it for Redis (`nestrs-redis`) changes this module only.
#[module(
    imports = [JobsModule::in_memory()],
    providers = [QueuePublisher, Announced, PostPublishedHandler],
    exports = [OutboxPublisherPort, Announced],
)]
pub struct MessagingModule;

/// The relay that moves recorded announcements to the queue. It only runs when there is a
/// database to read them from.
pub fn outbox_relay() -> ModuleRef {
    let config = Provider::factory::<OutboxConfig>(vec![Token::of::<AppConfig>()], |container| {
        Box::pin(async move {
            let app = container.resolve::<AppConfig>()?;
            Ok(OutboxConfig {
                enabled: app.storage == Storage::Database,
                ..OutboxConfig::default()
            })
        })
    });
    OutboxModule::with_config(
        config,
        [ModuleRef::of::<PersistenceModule>(), ModuleRef::of::<MessagingModule>()],
    )
}
