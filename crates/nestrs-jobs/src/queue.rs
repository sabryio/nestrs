//! Background queues: ports, the producer facade and the in-memory backend.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nestrs::prelude::*;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

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
}

/// An [`Envelope`] a worker holds exclusively until it acks or nacks it.
#[derive(Debug, Clone)]
pub struct Reserved {
    /// The work.
    pub envelope: Envelope,
    /// Backend-private handle used to ack or nack; opaque to callers.
    pub receipt: String,
}

/// Port: durable FIFO queues with explicit acknowledgement (at-least-once).
///
/// Contract: an envelope returned by `reserve` is invisible to other workers until it is
/// `ack`ed (gone), `retry`ed (back on the queue with new attempts) or `bury`ed (moved to
/// the dead letters). A worker that crashes holding one must not lose it: durable
/// backends hand it out again after [`recover`](Self::recover).
#[port]
pub trait QueueBackend {
    /// Appends work.
    async fn push(&self, queue: &str, envelope: Envelope) -> Result<(), QueueError>;

    /// Takes the oldest work, waiting up to `wait` for some to arrive. `None` on timeout.
    async fn reserve(&self, queue: &str, wait: Duration) -> Result<Option<Reserved>, QueueError>;

    /// Done: forget it.
    async fn ack(&self, queue: &str, reserved: &Reserved) -> Result<(), QueueError>;

    /// Failed, try again: put `envelope` (with its attempt count updated) back.
    async fn retry(&self, queue: &str, reserved: &Reserved, envelope: Envelope) -> Result<(), QueueError>;

    /// Failed for good: keep it in the dead letters for inspection.
    async fn bury(&self, queue: &str, reserved: &Reserved) -> Result<(), QueueError>;

    /// Makes everything reserved by workers that no longer exist available again.
    /// Called once at startup, before any worker reserves.
    async fn recover(&self, queue: &str) -> Result<usize, QueueError>;

    /// How many envelopes are waiting (not counting reserved or dead ones).
    async fn len(&self, queue: &str) -> Result<usize, QueueError>;

    /// How many envelopes were buried.
    async fn dead_len(&self, queue: &str) -> Result<usize, QueueError>;
}

/// The producer's side: serialises a payload and enqueues it.
#[injectable]
pub struct Queue {
    backend: Arc<QueueBackendPort>,
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
    reserved: HashMap<String, Envelope>,
    dead: Vec<Envelope>,
}

/// A queue backend for one process: no persistence, but the same contract.
#[derive(Default)]
pub struct MemoryQueueBackend {
    queues: Mutex<HashMap<String, MemoryQueue>>,
    arrived: Notify,
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

    async fn reserve(&self, queue: &str, wait: Duration) -> Result<Option<Reserved>, QueueError> {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            // Register interest before checking, so a push between the two is not missed.
            let notified = self.arrived.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let taken = self.with(queue, |q| {
                q.waiting.pop_front().inspect(|envelope| {
                    q.reserved.insert(envelope.id.clone(), envelope.clone());
                })
            });
            if let Some(envelope) = taken {
                let receipt = envelope.id.clone();
                return Ok(Some(Reserved { envelope, receipt }));
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return Ok(None);
            }
        }
    }

    async fn ack(&self, queue: &str, reserved: &Reserved) -> Result<(), QueueError> {
        self.with(queue, |q| q.reserved.remove(&reserved.receipt));
        Ok(())
    }

    async fn retry(&self, queue: &str, reserved: &Reserved, envelope: Envelope) -> Result<(), QueueError> {
        self.with(queue, |q| {
            q.reserved.remove(&reserved.receipt);
            q.waiting.push_back(envelope);
        });
        self.arrived.notify_waiters();
        Ok(())
    }

    async fn bury(&self, queue: &str, reserved: &Reserved) -> Result<(), QueueError> {
        self.with(queue, |q| {
            if let Some(envelope) = q.reserved.remove(&reserved.receipt) {
                q.dead.push(envelope);
            }
        });
        Ok(())
    }

    async fn recover(&self, queue: &str) -> Result<usize, QueueError> {
        let count = self.with(queue, |q| {
            let stranded: Vec<Envelope> = q.reserved.drain().map(|(_, envelope)| envelope).collect();
            let count = stranded.len();
            q.waiting.extend(stranded);
            count
        });
        self.arrived.notify_waiters();
        Ok(count)
    }

    async fn len(&self, queue: &str) -> Result<usize, QueueError> {
        Ok(self.with(queue, |q| q.waiting.len()))
    }

    async fn dead_len(&self, queue: &str) -> Result<usize, QueueError> {
        Ok(self.with(queue, |q| q.dead.len()))
    }
}
