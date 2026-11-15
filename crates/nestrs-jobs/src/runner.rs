//! Runs jobs and queue workers for the life of the app.

use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::FutureExt;
use nestrs::prelude::*;
use tokio::sync::watch;
use tokio::task::JoinSet;

use crate::job::Job;
use crate::queue::{Envelope, HandlerError, QueueHandler, Reserved};

/// Tuning for the runner.
#[derive(Debug, Clone)]
pub struct JobsConfig {
    /// Start nothing at all. Lets a deployment run the web tier without background work
    /// (and a separate worker deployment run only that).
    pub enabled: bool,
    /// How long shutdown waits for running work before abandoning it.
    pub shutdown_grace: Duration,
    /// How long a worker waits on an empty queue before checking for shutdown.
    pub poll: Duration,
    /// Concurrent workers per queue handler.
    pub concurrency: usize,
    /// How long a worker holds work before it counts as dead. Workers renew it every
    /// third of this while they run, so it only has to outlast a stall, not a job.
    pub lease: Duration,
    /// How often expired leases are looked for and their work requeued.
    pub reap_every: Duration,
}

impl Default for JobsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            shutdown_grace: Duration::from_secs(10),
            poll: Duration::from_secs(1),
            concurrency: 1,
            lease: Duration::from_secs(30),
            reap_every: Duration::from_secs(5),
        }
    }
}

struct Running {
    stop: watch::Sender<bool>,
    tasks: JoinSet<()>,
}

/// Owns every background task: starts them once the graph is built, stops them (within
/// the grace period) before the providers they use are destroyed.
#[injectable]
pub struct JobRunner {
    jobs: Arc<Multi<dyn Job>>,
    handlers: Arc<Multi<dyn QueueHandler>>,
    lock: Arc<crate::JobLockPort>,
    backend: Arc<crate::QueueBackendPort>,
    config: Arc<JobsConfig>,
    #[inject(default)]
    running: Mutex<Option<Running>>,
}

impl OnModuleInit for JobRunner {
    async fn on_module_init(&self) -> Result<(), DiError> {
        if !self.config.enabled {
            tracing::info!("background jobs are disabled");
            return Ok(());
        }
        let (stop, signal) = watch::channel(false);
        let mut tasks = JoinSet::new();

        for job in self.jobs.iter() {
            tasks.spawn(run_schedule(job.clone(), self.lock.clone(), signal.clone()));
        }
        for handler in self.handlers.iter() {
            tasks.spawn(run_reaper(
                handler.clone(),
                self.backend.clone(),
                self.config.reap_every,
                signal.clone(),
            ));
            for _ in 0..self.config.concurrency.max(1) {
                tasks.spawn(run_worker(
                    handler.clone(),
                    self.backend.clone(),
                    (*self.config).clone(),
                    signal.clone(),
                ));
            }
        }
        tracing::info!(
            jobs = self.jobs.len(),
            queues = self.handlers.len(),
            "background tasks started"
        );
        *self.running.lock().unwrap_or_else(|p| p.into_inner()) = Some(Running { stop, tasks });
        Ok(())
    }
}

impl OnModuleDestroy for JobRunner {
    async fn on_module_destroy(&self) -> Result<(), DiError> {
        let running = self.running.lock().unwrap_or_else(|p| p.into_inner()).take();
        let Some(Running { stop, mut tasks }) = running else {
            return Ok(());
        };
        let _ = stop.send(true);
        let drained = tokio::time::timeout(self.config.shutdown_grace, async {
            while tasks.join_next().await.is_some() {}
        })
        .await;
        if drained.is_err() {
            tracing::warn!("background tasks did not finish within the grace period; abandoning them");
            tasks.abort_all();
        }
        Ok(())
    }
}

/// Runs one job on its schedule until `stop`.
async fn run_schedule(job: Arc<dyn Job>, lock: Arc<crate::JobLockPort>, mut stop: watch::Receiver<bool>) {
    let name = job.name().to_owned();
    let schedule = job.schedule();
    loop {
        let now = chrono::Utc::now();
        let Some(delay) = schedule.next_delay(now) else {
            tracing::info!(job = %name, "schedule has no future runs; stopping");
            return;
        };
        let period = schedule.period(now);
        tokio::select! {
            biased;
            _ = stop.wait_for(|stopped| *stopped) => return,
            () = tokio::time::sleep(delay) => {}
        }
        // Shorter than the period, so the next tick is free to be claimed again.
        let ttl = period.mul_f32(0.9).max(Duration::from_millis(1));
        match lock.try_acquire(&name, ttl).await {
            Ok(true) => {}
            Ok(false) => {
                tracing::debug!(job = %name, "another instance runs this tick");
                continue;
            }
            Err(error) => {
                tracing::warn!(job = %name, %error, "skipping the tick: cannot take the lock");
                continue;
            }
        }
        let attempt = AssertUnwindSafe(tokio::time::timeout(job.timeout(), job.run())).catch_unwind();
        match attempt.await {
            Ok(Ok(Ok(()))) => tracing::debug!(job = %name, "job finished"),
            Ok(Ok(Err(error))) => tracing::error!(job = %name, %error, "job failed"),
            Ok(Err(_)) => tracing::error!(job = %name, timeout = ?job.timeout(), "job timed out"),
            Err(_) => tracing::error!(job = %name, "job panicked"),
        }
    }
}

/// Puts expired leases back on the queue until `stop`.
async fn run_reaper(
    handler: Arc<dyn QueueHandler>,
    backend: Arc<crate::QueueBackendPort>,
    every: Duration,
    mut stop: watch::Receiver<bool>,
) {
    let queue = handler.queue().to_owned();
    loop {
        tokio::select! {
            biased;
            _ = stop.wait_for(|stopped| *stopped) => return,
            () = tokio::time::sleep(every) => {}
        }
        match backend.reap(&queue, handler.max_attempts()).await {
            Ok(0) => {}
            Ok(count) => tracing::warn!(%queue, count, "work whose worker stopped renewing its lease was returned"),
            Err(error) => tracing::warn!(%queue, %error, "cannot reap expired leases"),
        }
    }
}

/// Consumes one queue until `stop`: reserve, handle, then ack, retry or bury.
async fn run_worker(
    handler: Arc<dyn QueueHandler>,
    backend: Arc<crate::QueueBackendPort>,
    config: JobsConfig,
    mut stop: watch::Receiver<bool>,
) {
    let queue = handler.queue().to_owned();
    loop {
        let reserved = tokio::select! {
            biased;
            _ = stop.wait_for(|stopped| *stopped) => return,
            reserved = backend.reserve(&queue, config.poll, config.lease) => reserved,
        };
        let reserved = match reserved {
            Ok(Some(reserved)) => reserved,
            Ok(None) => continue,
            Err(error) => {
                tracing::warn!(%queue, %error, "cannot reserve work; backing off");
                tokio::select! {
                    _ = stop.wait_for(|stopped| *stopped) => return,
                    () = tokio::time::sleep(config.poll) => continue,
                }
            }
        };
        // Work that has begun is finished even if shutdown arrives: the grace period,
        // not this loop, bounds it.
        settle(&queue, handler.as_ref(), backend.as_ref(), &config, reserved, &mut stop).await;
    }
}

/// Runs `work` while renewing the lease on `reserved`, so a long job is not mistaken for
/// a dead worker. A lost lease is logged once; the work still runs to its end.
async fn leased<T>(
    queue: &str,
    backend: &crate::QueueBackendPort,
    reserved: &Reserved,
    lease: Duration,
    work: impl std::future::Future<Output = T>,
) -> T {
    tokio::pin!(work);
    let mut ticker = tokio::time::interval((lease / 3).max(Duration::from_millis(1)));
    ticker.tick().await; // the first tick is immediate
    let mut warned = false;
    loop {
        tokio::select! {
            result = &mut work => return result,
            _ = ticker.tick() => match backend.renew(queue, reserved, lease).await {
                Ok(true) => {}
                Ok(false) if !warned => {
                    warned = true;
                    tracing::warn!(%queue, id = %reserved.envelope.id, "lease lost; this work may run elsewhere too");
                }
                Ok(false) => {}
                Err(error) => tracing::warn!(%queue, %error, "cannot renew a lease"),
            },
        }
    }
}

async fn settle(
    queue: &str,
    handler: &dyn QueueHandler,
    backend: &crate::QueueBackendPort,
    config: &JobsConfig,
    reserved: Reserved,
    stop: &mut watch::Receiver<bool>,
) {
    let id = reserved.envelope.id.clone();
    let handled = AssertUnwindSafe(tokio::time::timeout(
        handler.timeout(),
        handler.handle(&reserved.envelope.payload),
    ))
    .catch_unwind();
    let attempt = leased(queue, backend, &reserved, config.lease, handled).await;
    let outcome = match attempt {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err(HandlerError::Retry("the attempt timed out".to_owned())),
        Err(_) => Err(HandlerError::Retry("the handler panicked".to_owned())),
    };
    let attempts = reserved.envelope.attempts + 1;
    let result = match outcome {
        Ok(()) => backend.ack(queue, &reserved).await,
        Err(HandlerError::Fatal(reason)) => {
            tracing::error!(%queue, %id, %reason, "work buried: it cannot succeed");
            backend.bury(queue, &reserved).await
        }
        Err(HandlerError::Retry(reason)) if attempts >= handler.max_attempts() => {
            tracing::error!(%queue, %id, attempts, %reason, "work buried: out of attempts");
            backend.bury(queue, &reserved).await
        }
        Err(HandlerError::Retry(reason)) => {
            tracing::warn!(%queue, %id, attempts, %reason, "attempt failed; will retry");
            // Keep the lease while waiting, so the reaper does not count the backoff as a
            // crash; leave early on shutdown.
            let wait = async {
                tokio::select! {
                    _ = stop.wait_for(|stopped| *stopped) => {}
                    () = tokio::time::sleep(handler.backoff(attempts)) => {}
                }
            };
            leased(queue, backend, &reserved, config.lease, wait).await;
            let envelope = Envelope {
                attempts,
                ..reserved.envelope.clone()
            };
            backend.retry(queue, &reserved, envelope).await
        }
    };
    if let Err(error) = result {
        tracing::error!(%queue, %id, %error, "cannot settle work; its lease will expire and it will be reaped");
    }
}
