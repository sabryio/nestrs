//! # nestrs-jobs
//!
//! Scheduled tasks and background queues.
//!
//! ```ignore
//! #[module(imports = [JobsModule::in_memory()], providers = [PurgeDrafts, SendWelcomeEmail], ..)]
//! struct AppModule;
//!
//! // a recurring job, from any module
//! #[injectable(contributes = JobPort)]
//! struct PurgeDrafts { .. }
//!
//! // a queue consumer, from any module
//! #[injectable(contributes = QueueHandlerPort)]
//! struct SendWelcomeEmail { .. }
//!
//! // a producer: inject `Queue`
//! self.queue.enqueue("welcome-emails", &Welcome { user }).await?;
//! ```
//!
//! ## Guarantees
//!
//! * **Jobs** run on an interval or cron schedule (UTC). A [`JobLock`] makes one instance
//!   run each tick; a job never overlaps itself; a panic or timeout is a logged failure.
//! * **Queues** deliver at-least-once: work is reserved, then acked, retried with
//!   exponential backoff, or buried after `max_attempts` (or at once for a
//!   [`HandlerError::Fatal`]). Work held by a crashed worker is recovered at startup.
//! * Everything stops within [`JobsConfig::shutdown_grace`] when the app shuts down.
//!
//! ## Design
//!
//! * [`Job`], [`QueueHandler`] are the features' ports; [`JobLock`] and [`QueueBackend`]
//!   are the infrastructure ports. The runner knows only ports (DIP), and adding a job
//!   or consumer never edits this crate (OCP).
//! * [`Schedule`] (when), [`Queue`] (produce), [`JobRunner`] (execute) each have one
//!   reason to change (SRP). Every backend honours the same documented contract (LSP),
//!   and the two infrastructure ports are separate so an app can mix them (ISP).

#![forbid(unsafe_code)]

mod job;
mod lock;
mod module;
mod queue;
mod runner;
mod schedule;
mod trace;

pub use job::{Job, JobError, JobPort};
pub use lock::{JobLock, JobLockPort, LockError, MemoryJobLock};
pub use module::JobsModule;
pub use queue::{
    Envelope, HandlerError, MemoryQueueBackend, Queue, QueueBackend, QueueBackendPort, QueueError, QueueHandler,
    QueueHandlerPort, Reserved, decode,
};
pub use runner::{JobRunner, JobsConfig};
pub use schedule::{Schedule, ScheduleError};
pub use trace::{TracePropagation, TracePropagationPort};
