//! Packages the runner as a module.

use purlin::prelude::*;

use crate::job::Job;
use crate::lock::MemoryJobLock;
use crate::queue::{MemoryQueueBackend, Queue, QueueHandler};
use crate::runner::{JobRunner, JobsConfig};

/// Import it to run [`Job`]s and [`QueueHandler`]s contributed anywhere in the app.
///
/// The lock and queue backends are *ports* the app chooses:
///
/// * [`in_memory`](Self::in_memory): single instance, no dependencies.
/// * [`with_backends`](Self::with_backends): pass providers for `JobLockPort` and
///   `QueueBackendPort`, e.g. the Redis adapters from `purlin-redis`.
///
/// It exports [`Queue`], so any module that imports this one can enqueue work.
pub struct JobsModule;

impl JobsModule {
    /// Memory-backed lock and queues, default settings.
    pub fn in_memory() -> ModuleRef {
        Self::in_memory_with(JobsConfig::default())
    }

    /// Memory-backed lock and queues with your own settings (concurrency, leases, trace propagation).
    pub fn in_memory_with(config: JobsConfig) -> ModuleRef {
        Self::build(
            config,
            Provider::value(std::sync::Arc::new(MemoryJobLock::new()) as crate::JobLockPort),
            Provider::value(std::sync::Arc::new(MemoryQueueBackend::new()) as crate::QueueBackendPort),
        )
    }

    /// Your own `lock` and `queue` providers and settings.
    pub fn with_backends(config: JobsConfig, lock: Provider, queue: Provider) -> ModuleRef {
        Self::build(config, lock, queue)
    }

    fn build(config: JobsConfig, lock: Provider, queue: Provider) -> ModuleRef {
        ModuleDef::new("JobsModule")
            .providers([
                Provider::value(config),
                lock,
                queue,
                Provider::multi::<dyn Job>(),
                Provider::multi::<dyn QueueHandler>(),
                Provider::of::<Queue>(),
                Provider::of::<JobRunner>()
                    .with_init::<JobRunner>()
                    .with_destroy::<JobRunner>(),
            ])
            .exports([Token::of::<Queue>()])
            .into_ref()
    }
}
