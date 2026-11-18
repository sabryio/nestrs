//! Background queues: ports, the producer facade and the in-memory backend.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use purlin::prelude::*;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use tokio::time::Instant;

/// The queue backend failed or refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum QueueError {
    /// The backend could not be reached or answered with an error.
    #[error("queue backend unavailable: {0}")]
    Backend(String),
    /// A payload could not be encoded.
    #[error("cannot encode payload: {0}")]
    Encode(String),
}

/// One unit of work as stored: an id, the JSON payload and how often it was tried.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    /// Unique per enqueue.
    pub id: String,
    /// The JSON payload.
    pub payload: String,
    /// Completed attempts so far.
    pub attempts: u32,
    /// W3C `traceparent` of the producer, so the handler's span joins its trace. Present only
    /// when a [`TracePropagation`](crate::TracePropagation) is configured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub traceparent: Option<String>,
}

/// An [`Envelope`] a worker holds exclusively until it acks or nacks it.
#[derive(Debug, Clone)]
pub struct Reserved {
    /// The work.
    pub envelope: Envelope,
    /// Backend-private handle used to ack or nack; opaque to callers.
    pub receipt: String,
}

/// Port: durable FIFO queues with leased delivery (at-least-once).
///
/// A worker `reserve`s work for a `lease`. While it holds the lease nobody else gets the
/// work; it must `renew` the lease if it needs longer, and finish with `ack` (done),
/// `retry` (back on the queue) or `bury` (dead letters). If the worker dies, the lease
/// runs out and [`reap`](Self::reap) puts the work back, so recovery needs no restart
/// and never touches work a live worker still holds.
///
/// Contract:
/// * `reserve` returns the oldest waiting work, hidden from others until its lease ends.
/// * `renew` returns `false` if the lease is already gone (the work was reaped and may
///   be running elsewhere); the holder should expect a duplicate.
/// * `ack`, `retry` and `bury` act only while the lease is still held; after it was
///   lost they do nothing, so a late worker cannot duplicate or erase work that has
///   moved on.
/// * `reap` returns expired work to the queue with its attempt count raised by one (a
///   worker that died mid-run did try). Work whose count would reach `max_attempts` goes
///   to the dead letters instead, so work that kills its worker cannot loop forever.
#[port]
pub trait QueueBackend {
    /// Appends work.
    async fn push(&self, queue: &str, envelope: Envelope) -> Result<(), QueueError>;

    /// Takes the oldest work under a `lease`, waiting up to `wait` for some to arrive.
    async fn reserve(&self, queue: &str, wait: Duration, lease: Duration) -> Result<Option<Reserved>, QueueError>;

    /// Extends the lease to `lease` from now. `false` if it was already lost.
    async fn renew(&self, queue: &str, reserved: &Reserved, lease: Duration) -> Result<bool, QueueError>;

    /// Done: forget it.
    async fn ack(&self, queue: &str, reserved: &Reserved) -> Result<(), QueueError>;

    /// Failed, try again: put `envelope` (with its attempt count updated) back.
    async fn retry(&self, queue: &str, reserved: &Reserved, envelope: Envelope) -> Result<(), QueueError>;

    /// Failed for good: keep it in the dead letters for inspection.
    async fn bury(&self, queue: &str, reserved: &Reserved) -> Result<(), QueueError>;

    /// Returns work whose lease expired to the queue (or the dead letters, see above).
    /// Returns how many were requeued or buried.
    async fn reap(&self, queue: &str, max_attempts: u32) -> Result<usize, QueueError>;

    /// How many envelopes are waiting (not counting leased or dead ones).
    async fn len(&self, queue: &str) -> Result<usize, QueueError>;

    /// How many envelopes are currently leased.
    async fn leased_len(&self, queue: &str) -> Result<usize, QueueError>;

    /// How many envelopes were buried.
    async fn dead_len(&self, queue: &str) -> Result<usize, QueueError>;
}

/// The producer's side: serialises a payload and enqueues it.
#[injectable]
pub struct Queue {
    backend: Arc<QueueBackendPort>,
    config: Arc<crate::JobsConfig>,
}

impl Queue {
    /// Enqueues `payload` on `queue`; returns the job id.
    pub async fn enqueue<T: Serialize>(&self, queue: &str, payload: &T) -> Result<String, QueueError> {
        let payload = serde_json::to_string(payload).map_err(|error| QueueError::Encode(error.to_string()))?;
        let id = uuid::Uuid::new_v4().to_string();
        self.backend
            .push(
                queue,
                Envelope {
                    id: id.clone(),
                    payload,
                    attempts: 0,
                    traceparent: self
                        .config
                        .propagation
                        .as_ref()
                        .and_then(|propagation| propagation.current()),
                },
            )
            .await?;
        Ok(id)
    }
}

/// A handler could not process a payload.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HandlerError {
    /// Worth retrying (a downstream was down).
    #[error("{0}")]
    Retry(String),
    /// Retrying cannot help (the payload is invalid); goes straight to the dead letters.
    #[error("{0}")]
    Fatal(String),
}

/// Port: consumes one queue.
///
/// Contribute implementations from any module; a worker is started for each:
///
/// ```ignore
/// #[injectable(contributes = QueueHandlerPort)]
/// struct SendWelcomeEmail { mailer: Arc<MailerPort> }
///
/// #[async_trait]
/// impl QueueHandler for SendWelcomeEmail {
///     fn queue(&self) -> &str { "welcome-emails" }
///     async fn handle(&self, payload: &str) -> Result<(), HandlerError> {
///         let job: Welcome = decode(payload)?;
///         self.mailer.send(job).await.map_err(|e| HandlerError::Retry(e.to_string()))
///     }
/// }
/// ```
///
/// Delivery is at-least-once, so `handle` must be idempotent.
#[port]
pub trait QueueHandler {
    /// The queue consumed.
    fn queue(&self) -> &str;

    /// Attempts before the work is buried. Defaults to 5.
    fn max_attempts(&self) -> u32 {
        5
    }

    /// Longest one attempt may take. Defaults to one minute.
    fn timeout(&self) -> Duration {
        Duration::from_secs(60)
    }

    /// Wait before attempt `attempt + 1` after `attempt` failures: exponential from one
    /// second, capped at five minutes.
    fn backoff(&self, attempt: u32) -> Duration {
        Duration::from_secs(1u64 << attempt.saturating_sub(1).min(8)).min(Duration::from_secs(300))
    }

    /// Processes one payload.
    async fn handle(&self, payload: &str) -> Result<(), HandlerError>;
}

/// Decodes a handler's JSON payload; a malformed one is [`HandlerError::Fatal`].
pub fn decode<T: DeserializeOwned>(payload: &str) -> Result<T, HandlerError> {
    serde_json::from_str(payload).map_err(|error| HandlerError::Fatal(format!("invalid payload: {error}")))
}

#[derive(Default)]
struct MemoryQueue {
    waiting: VecDeque<Envelope>,
    /// id → (work, lease end, reservation number). The number tells a current holder from a
    /// previous one that lost its lease.
    leased: HashMap<String, (Envelope, Instant, u64)>,
    dead: Vec<Envelope>,
}

/// A queue backend for one process: no persistence, but the same contract.
#[derive(Default)]
pub struct MemoryQueueBackend {
    queues: Mutex<HashMap<String, MemoryQueue>>,
    arrived: Notify,
    reservations: std::sync::atomic::AtomicU64,
}

/// Splits `id#number`.
fn holder(receipt: &str) -> (&str, u64) {
    let (id, number) = receipt.rsplit_once('#').unwrap_or((receipt, "0"));
    (id, number.parse().unwrap_or(0))
}

impl MemoryQueueBackend {
    /// An empty backend.
    pub fn new() -> Self {
        Self::default()
    }

    fn with<R>(&self, queue: &str, f: impl FnOnce(&mut MemoryQueue) -> R) -> R {
        let mut queues = self.queues.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        f(queues.entry(queue.to_owned()).or_default())
    }
}

#[async_trait]
impl QueueBackend for MemoryQueueBackend {
    async fn push(&self, queue: &str, envelope: Envelope) -> Result<(), QueueError> {
        self.with(queue, |q| q.waiting.push_back(envelope));
        self.arrived.notify_waiters();
        Ok(())
    }

    async fn reserve(&self, queue: &str, wait: Duration, lease: Duration) -> Result<Option<Reserved>, QueueError> {
        let deadline = Instant::now() + wait;
        loop {
            // Register interest before checking, so a push between the two is not missed.
            let notified = self.arrived.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let number = self.reservations.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let taken = self.with(queue, |q| {
                q.waiting.pop_front().inspect(|envelope| {
                    q.leased
                        .insert(envelope.id.clone(), (envelope.clone(), Instant::now() + lease, number));
                })
            });
            if let Some(envelope) = taken {
                let receipt = format!("{}#{number}", envelope.id);
                return Ok(Some(Reserved { envelope, receipt }));
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return Ok(None);
            }
        }
    }

    async fn renew(&self, queue: &str, reserved: &Reserved, lease: Duration) -> Result<bool, QueueError> {
        let (id, number) = holder(&reserved.receipt);
        Ok(self.with(queue, |q| match q.leased.get_mut(id) {
            Some((_, until, current)) if *current == number => {
                *until = Instant::now() + lease;
                true
            }
            _ => false,
        }))
    }

    async fn ack(&self, queue: &str, reserved: &Reserved) -> Result<(), QueueError> {
        let (id, number) = holder(&reserved.receipt);
        self.with(queue, |q| {
            if q.leased.get(id).is_some_and(|(_, _, current)| *current == number) {
                q.leased.remove(id);
            }
        });
        Ok(())
    }

    async fn retry(&self, queue: &str, reserved: &Reserved, envelope: Envelope) -> Result<(), QueueError> {
        let (id, number) = holder(&reserved.receipt);
        let held = self.with(queue, |q| {
            let held = q.leased.get(id).is_some_and(|(_, _, current)| *current == number);
            if held {
                q.leased.remove(id);
                q.waiting.push_back(envelope);
            }
            held
        });
        if held {
            self.arrived.notify_waiters();
        }
        Ok(())
    }

    async fn bury(&self, queue: &str, reserved: &Reserved) -> Result<(), QueueError> {
        let (id, number) = holder(&reserved.receipt);
        self.with(queue, |q| {
            if q.leased.get(id).is_some_and(|(_, _, current)| *current == number)
                && let Some((envelope, ..)) = q.leased.remove(id)
            {
                q.dead.push(envelope);
            }
        });
        Ok(())
    }

    async fn reap(&self, queue: &str, max_attempts: u32) -> Result<usize, QueueError> {
        let now = Instant::now();
        let count = self.with(queue, |q| {
            let expired: Vec<String> = q
                .leased
                .iter()
                .filter(|(_, (_, until, _))| *until <= now)
                .map(|(id, _)| id.clone())
                .collect();
            for id in &expired {
                if let Some((mut envelope, ..)) = q.leased.remove(id) {
                    envelope.attempts += 1;
                    if envelope.attempts >= max_attempts {
                        q.dead.push(envelope);
                    } else {
                        q.waiting.push_back(envelope);
                    }
                }
            }
            expired.len()
        });
        if count > 0 {
            self.arrived.notify_waiters();
        }
        Ok(count)
    }

    async fn len(&self, queue: &str) -> Result<usize, QueueError> {
        Ok(self.with(queue, |q| q.waiting.len()))
    }

    async fn leased_len(&self, queue: &str) -> Result<usize, QueueError> {
        Ok(self.with(queue, |q| q.leased.len()))
    }

    async fn dead_len(&self, queue: &str) -> Result<usize, QueueError> {
        Ok(self.with(queue, |q| q.dead.len()))
    }
}
