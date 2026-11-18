//! The port a scheduled task implements.

use std::time::Duration;

use purlin::prelude::*;

use crate::schedule::Schedule;

/// A job failed; the scheduler logs it and runs the next tick as usual.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct JobError(pub String);

impl JobError {
    /// A failure with a reason for the log.
    pub fn new(reason: impl Into<String>) -> Self {
        Self(reason.into())
    }
}

/// Port: work that runs on a [`Schedule`].
///
/// Contribute implementations from any module; the scheduler finds them all:
///
/// ```ignore
/// #[injectable(contributes = JobPort)]
/// struct PurgeExpiredDrafts { posts: Arc<PostsPort> }
///
/// #[async_trait]
/// impl Job for PurgeExpiredDrafts {
///     fn name(&self) -> &str { "purge-expired-drafts" }
///     fn schedule(&self) -> Schedule { Schedule::cron("0 0 * * * *").unwrap() }
///     async fn run(&self) -> Result<(), JobError> { self.posts.purge().await.map_err(|e| JobError::new(e.to_string())) }
/// }
/// ```
///
/// Contract: a job never overlaps itself inside one process (the next wait starts when
/// a run ends), is cancelled after [`timeout`](Self::timeout), and a panic is a failed
/// run, not a crash. Across instances the [`JobLock`](crate::JobLock) decides who runs
/// each tick, so `run` should still be idempotent: locks bound duplicates, they cannot
/// promise exactly-once.
#[port]
pub trait Job {
    /// A stable, unique name: the lock key and the label in logs.
    fn name(&self) -> &str;

    /// When to run.
    fn schedule(&self) -> Schedule;

    /// Longest one run may take. Defaults to five minutes.
    fn timeout(&self) -> Duration {
        Duration::from_secs(300)
    }

    /// The work.
    async fn run(&self) -> Result<(), JobError>;
}
