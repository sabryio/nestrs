use nestrs::prelude::*;
use nestrs_jobs::{JobsConfig, JobsModule};
use nestrs_otel::OtelPropagation;
use nestrs_outbox::{OutboxConfig, OutboxModule, OutboxPublisherPort, QueuePublisher};

use super::PersistenceModule;
use crate::infrastructure::announcements::{Announced, PostPublishedHandler};
use crate::infrastructure::config::{AppConfig, Storage};

/// Where announcements go: a queue, and the consumer of that queue. The queue is in-process
/// here; swapping it for Redis (`nestrs-redis`) changes this module only.
#[module(
    imports = [JobsModule::in_memory_with(jobs_config())],
    providers = [QueuePublisher, Announced, PostPublishedHandler],
    exports = [OutboxPublisherPort, Announced],
)]
pub struct MessagingModule;

/// Queue settings. Trace context travels with every queued message, so a worker's span is a
/// child of the request that enqueued the work (a no-op when no telemetry is installed).
fn jobs_config() -> JobsConfig {
    JobsConfig {
        propagation: Some(std::sync::Arc::new(OtelPropagation::new())),
        ..JobsConfig::default()
    }
}

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
