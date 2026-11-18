//! The relay: moves recorded messages to their destination for the life of the app.

use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::FutureExt;
use purlin::prelude::*;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tracing::Instrument;

use crate::publisher::PublishError;
use crate::{OutboxMessage, OutboxPublisherPort, OutboxStorePort};

/// Tuning for the relay.
#[derive(Debug, Clone)]
pub struct OutboxConfig {
    /// Run the relay at all. Lets one deployment record messages and another deliver them.
    pub enabled: bool,
    /// How long to wait before looking again once the outbox is empty. This is the latency
    /// floor between a commit and delivery.
    pub poll_interval: Duration,
    /// Messages claimed per look.
    pub batch_size: usize,
    /// How long a claim lasts. If a relay dies, its messages become claimable again after
    /// this. It must outlast one batch (`batch_size` times `publish_timeout` in the worst
    /// case); a claim that expires early only causes a duplicate, never a loss.
    pub lease: Duration,
    /// Give up on one message after this many failed deliveries and bury it.
    pub max_attempts: u32,
    /// First retry delay; doubles with every failure.
    pub backoff_base: Duration,
    /// Ceiling of the retry delay.
    pub backoff_max: Duration,
    /// How long one `publish` may take before it counts as a failure.
    pub publish_timeout: Duration,
    /// How long shutdown waits for the batch in flight.
    pub shutdown_grace: Duration,
}

impl Default for OutboxConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            poll_interval: Duration::from_millis(500),
            batch_size: 20,
            lease: Duration::from_secs(60),
            max_attempts: 10,
            backoff_base: Duration::from_secs(1),
            backoff_max: Duration::from_secs(300),
            publish_timeout: Duration::from_secs(10),
            shutdown_grace: Duration::from_secs(10),
        }
    }
}

impl OutboxConfig {
    /// Delay before the retry that follows `failures` failed deliveries.
    pub fn backoff(&self, failures: u32) -> Duration {
        let factor = 2u32.saturating_pow(failures.min(20));
        self.backoff_base.saturating_mul(factor).min(self.backoff_max)
    }
}

/// What the relay has done since it started. Cheap to read; export it as metrics.
#[derive(Debug, Default)]
pub struct RelayStats {
    published: AtomicU64,
    retried: AtomicU64,
    buried: AtomicU64,
}

impl RelayStats {
    /// Messages delivered and removed.
    pub fn published(&self) -> u64 {
        self.published.load(Ordering::Relaxed)
    }
    /// Failed deliveries that were scheduled for another try.
    pub fn retried(&self) -> u64 {
        self.retried.load(Ordering::Relaxed)
    }
    /// Messages that became dead letters.
    pub fn buried(&self) -> u64 {
        self.buried.load(Ordering::Relaxed)
    }
}

/// Wall-clock milliseconds that advance with Tokio's clock, so tests on paused time see
/// retries and leases expire without sleeping for real.
#[derive(Clone, Copy)]
struct Clock {
    wall_ms: i64,
    origin: Instant,
}

impl Default for Clock {
    fn default() -> Self {
        Self::start()
    }
}

impl Clock {
    fn start() -> Self {
        let wall = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
        Self {
            wall_ms: i64::try_from(wall.as_millis()).unwrap_or(i64::MAX),
            origin: Instant::now(),
        }
    }

    fn now_ms(&self) -> i64 {
        let elapsed = i64::try_from(self.origin.elapsed().as_millis()).unwrap_or(i64::MAX);
        self.wall_ms.saturating_add(elapsed)
    }
}

/// The work of one relay, separate from lifecycle so it can be driven step by step.
struct Delivery {
    store: OutboxStorePort,
    publisher: OutboxPublisherPort,
    config: OutboxConfig,
    stats: Arc<RelayStats>,
    clock: Clock,
}

impl Delivery {
    /// Claims one batch and delivers it. Returns how many messages were handled.
    async fn tick(&self) -> usize {
        let now = self.clock.now_ms();
        let batch = match self.store.claim(now, self.config.batch_size, self.config.lease).await {
            Ok(batch) => batch,
            Err(error) => {
                tracing::warn!(%error, "outbox: cannot claim messages");
                return 0;
            }
        };
        let handled = batch.len();
        for message in batch {
            self.deliver(message).await;
        }
        handled
    }

    async fn deliver(&self, message: OutboxMessage) {
        let span = tracing::info_span!(
            "outbox.publish",
            otel.name = %format!("{} publish", message.topic),
            otel.kind = "producer",
            messaging.system = "purlin",
            messaging.destination.name = %message.topic,
            messaging.message.id = %message.id,
            messaging.attempt = message.attempts + 1,
            otel.status_code = tracing::field::Empty,
        );
        let attempt = tokio::time::timeout(
            self.config.publish_timeout,
            AssertUnwindSafe(self.publisher.publish(&message))
                .catch_unwind()
                .instrument(span.clone()),
        )
        .await;
        let outcome = match attempt {
            Err(_) => Err(PublishError::Retry("timed out".to_owned())),
            Ok(Err(_panic)) => Err(PublishError::Retry("the publisher panicked".to_owned())),
            Ok(Ok(result)) => result,
        };
        if outcome.is_err() {
            span.record("otel.status_code", "ERROR");
        }
        match outcome {
            Ok(()) => {
                self.record(self.store.complete(&message).await, &message, "complete");
                self.stats.published.fetch_add(1, Ordering::Relaxed);
            }
            Err(PublishError::Retry(reason)) if message.attempts + 1 < self.config.max_attempts => {
                let retry_at = self.clock.now_ms().saturating_add(
                    i64::try_from(self.config.backoff(message.attempts).as_millis()).unwrap_or(i64::MAX),
                );
                tracing::warn!(id = %message.id, topic = %message.topic, attempts = message.attempts + 1, %reason, "outbox: delivery failed, will retry");
                self.record(
                    self.store.release(&message, &reason, retry_at).await,
                    &message,
                    "release",
                );
                self.stats.retried.fetch_add(1, Ordering::Relaxed);
            }
            Err(PublishError::Retry(reason) | PublishError::Fatal(reason)) => {
                tracing::error!(id = %message.id, topic = %message.topic, %reason, "outbox: message buried");
                self.record(self.store.bury(&message, &reason).await, &message, "bury");
                self.stats.buried.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// A store call that failed or found the claim gone is logged; the message is safe either
    /// way (it will be claimed again), so nothing else is needed.
    fn record(&self, result: Result<bool, crate::StoreError>, message: &OutboxMessage, action: &str) {
        match result {
            Ok(true) => {}
            Ok(false) => {
                tracing::warn!(id = %message.id, action, "outbox: claim was lost before the update; the message may be delivered again");
            }
            Err(error) => {
                tracing::warn!(id = %message.id, action, %error, "outbox: store update failed; the message may be delivered again");
            }
        }
    }

    async fn run(self, mut stop: watch::Receiver<bool>) {
        loop {
            if *stop.borrow() {
                return;
            }
            if self.tick().await > 0 {
                // Work was found: look again at once, there may be more (and keyed messages
                // release their successors one at a time).
                continue;
            }
            tokio::select! {
                biased;
                _ = stop.wait_for(|stopped| *stopped) => return,
                () = tokio::time::sleep(self.config.poll_interval) => {}
            }
        }
    }
}

struct Running {
    stop: watch::Sender<bool>,
    task: JoinHandle<()>,
}

/// Delivers the outbox: claims messages, publishes them, removes them when delivered, retries
/// with exponential back-off and buries what cannot be delivered.
///
/// It starts when the module graph is up and stops (within
/// [`shutdown_grace`](OutboxConfig::shutdown_grace)) before the providers it uses are
/// destroyed. Several instances may run against one database: claims keep them apart.
#[injectable]
pub struct OutboxRelay {
    store: OutboxStorePort,
    publisher: OutboxPublisherPort,
    config: Arc<OutboxConfig>,
    #[inject(default)]
    stats: Arc<RelayStats>,
    #[inject(default)]
    clock: Clock,
    #[inject(default)]
    running: Mutex<Option<Running>>,
}

impl OutboxRelay {
    /// Builds a relay by hand (the module normally does it).
    pub fn new(store: OutboxStorePort, publisher: OutboxPublisherPort, config: OutboxConfig) -> Self {
        Self {
            store,
            publisher,
            config: Arc::new(config),
            stats: Arc::default(),
            clock: Clock::start(),
            running: Mutex::new(None),
        }
    }

    /// What the relay has done so far.
    pub fn stats(&self) -> &RelayStats {
        &self.stats
    }

    fn delivery(&self) -> Delivery {
        Delivery {
            store: Arc::clone(&self.store),
            publisher: Arc::clone(&self.publisher),
            config: (*self.config).clone(),
            stats: Arc::clone(&self.stats),
            clock: self.clock,
        }
    }

    /// Claims and delivers one batch now; returns how many messages were handled. The
    /// background task does this in a loop; tests call it to step deterministically.
    pub async fn deliver_once(&self) -> usize {
        self.delivery().tick().await
    }

    /// Starts the background task. Called by the lifecycle hook; a second call does nothing.
    pub fn start(&self) {
        let mut running = self.running.lock().unwrap_or_else(|p| p.into_inner());
        if running.is_some() {
            return;
        }
        let (stop, signal) = watch::channel(false);
        let task = tokio::spawn(self.delivery().run(signal));
        *running = Some(Running { stop, task });
    }

    /// Stops the background task, waiting up to the grace period for the batch in flight.
    pub async fn stop(&self) {
        let running = self.running.lock().unwrap_or_else(|p| p.into_inner()).take();
        let Some(Running { stop, mut task }) = running else {
            return;
        };
        let _ = stop.send(true);
        if tokio::time::timeout(self.config.shutdown_grace, &mut task)
            .await
            .is_err()
        {
            tracing::warn!("outbox: relay did not stop within the grace period; abandoning the batch in flight");
            task.abort();
        }
    }
}

impl OnModuleInit for OutboxRelay {
    async fn on_module_init(&self) -> Result<(), DiError> {
        if self.config.enabled {
            self.start();
            tracing::info!("outbox relay started");
        } else {
            tracing::info!("outbox relay is disabled");
        }
        Ok(())
    }
}

impl OnModuleDestroy for OutboxRelay {
    async fn on_module_destroy(&self) -> Result<(), DiError> {
        self.stop().await;
        Ok(())
    }
}
