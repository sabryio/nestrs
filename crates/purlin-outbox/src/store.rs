//! Where messages wait: the store port and an in-memory implementation.

use std::sync::Mutex;
use std::time::Duration;

use purlin::prelude::*;

use crate::{NewMessage, OutboxMessage, StoreError};

/// Port: the relay's view of the outbox.
///
/// Recording a message is *not* here: it has to happen inside the caller's database
/// transaction, so it is a method of the concrete store (see `Outbox` for SeaORM).
///
/// Contract, which every implementation must pass (`tests/store_contract.rs` runs the same
/// suite against each):
///
/// * `claim` returns up to `limit` messages in recording order that are due
///   (`available_at <= now`), not claimed by someone whose lease is still running, and not
///   blocked: a message with a key waits while an **older message with the same key is still
///   pending** (waiting, claimed or backing off). At most one message per key is returned
///   per call.
/// * `complete`, `release` and `bury` act only while the caller still holds the claim they
///   name; after the claim was lost (lease expired, someone else claimed) they do nothing
///   and return `false`.
/// * A buried message stops blocking its key and is kept for inspection.
#[port]
pub trait OutboxStore {
    /// Claims due messages for `lease`.
    async fn claim(&self, now_ms: i64, limit: usize, lease: Duration) -> Result<Vec<OutboxMessage>, StoreError>;

    /// Delivered: forget it. `false` if the claim was lost.
    async fn complete(&self, message: &OutboxMessage) -> Result<bool, StoreError>;

    /// Delivery failed: count the attempt, remember `error`, try again at `retry_at_ms`.
    async fn release(&self, message: &OutboxMessage, error: &str, retry_at_ms: i64) -> Result<bool, StoreError>;

    /// Delivery failed for good: keep it as a dead letter.
    async fn bury(&self, message: &OutboxMessage, error: &str) -> Result<bool, StoreError>;

    /// Messages that are not yet delivered or buried.
    async fn pending_len(&self) -> Result<usize, StoreError>;

    /// Messages that were buried.
    async fn dead_len(&self) -> Result<usize, StoreError>;

    /// Puts a dead letter back in the queue with a fresh attempt count. `false` if there is
    /// no such dead letter.
    async fn requeue_dead(&self, id: &str) -> Result<bool, StoreError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Pending,
    Dead,
}

#[derive(Debug, Clone)]
struct Row {
    seq: u64,
    id: String,
    topic: String,
    key: Option<String>,
    payload: String,
    attempts: u32,
    created_at_ms: i64,
    available_at_ms: i64,
    claimed_until_ms: Option<i64>,
    claim: Option<String>,
    status: Status,
    last_error: Option<String>,
}

#[derive(Default)]
struct Inner {
    next_seq: u64,
    next_claim: u64,
    rows: Vec<Row>,
}

/// An outbox in process memory.
///
/// For tests and for wiring the relay before a database exists. It is **not** transactional:
/// a message recorded here is not tied to any other write, which is the one thing an outbox
/// is for. Never use it where losing the pairing matters.
#[derive(Default)]
pub struct MemoryOutboxStore {
    inner: Mutex<Inner>,
}

impl MemoryOutboxStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a message; returns its id.
    pub fn push(&self, message: NewMessage) -> String {
        let mut inner = self.lock();
        inner.next_seq += 1;
        let id = format!("mem-{}", inner.next_seq);
        let row = Row {
            seq: inner.next_seq,
            id: id.clone(),
            topic: message.topic,
            key: message.key,
            payload: message.payload,
            attempts: 0,
            created_at_ms: 0,
            available_at_ms: 0,
            claimed_until_ms: None,
            claim: None,
            status: Status::Pending,
            last_error: None,
        };
        inner.rows.push(row);
        id
    }

    /// The last error recorded for a message, if it is still stored.
    pub fn last_error(&self, id: &str) -> Option<String> {
        self.lock()
            .rows
            .iter()
            .find(|row| row.id == id)
            .and_then(|row| row.last_error.clone())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn holds(row: &Row, message: &OutboxMessage) -> bool {
    row.id == message.id && row.claim.as_deref() == Some(message.claim.as_str())
}

#[async_trait]
impl OutboxStore for MemoryOutboxStore {
    async fn claim(&self, now_ms: i64, limit: usize, lease: Duration) -> Result<Vec<OutboxMessage>, StoreError> {
        let mut inner = self.lock();
        let lease_ms = i64::try_from(lease.as_millis()).unwrap_or(i64::MAX);
        let mut chosen = Vec::new();
        let mut keys_taken = std::collections::HashSet::new();
        for index in 0..inner.rows.len() {
            if chosen.len() >= limit {
                break;
            }
            let row = &inner.rows[index];
            let due = row.status == Status::Pending
                && row.available_at_ms <= now_ms
                && row.claimed_until_ms.is_none_or(|until| until <= now_ms);
            if !due {
                continue;
            }
            if let Some(key) = &row.key {
                let blocked = inner.rows.iter().any(|older| {
                    older.seq < row.seq && older.status == Status::Pending && older.key.as_ref() == Some(key)
                });
                if blocked || !keys_taken.insert(key.clone()) {
                    continue;
                }
            }
            chosen.push(index);
        }
        let mut claimed = Vec::new();
        for index in chosen {
            inner.next_claim += 1;
            let token = format!("claim-{}", inner.next_claim);
            let row = &mut inner.rows[index];
            row.claimed_until_ms = Some(now_ms.saturating_add(lease_ms));
            row.claim = Some(token.clone());
            claimed.push(OutboxMessage {
                id: row.id.clone(),
                topic: row.topic.clone(),
                key: row.key.clone(),
                payload: row.payload.clone(),
                attempts: row.attempts,
                created_at_ms: row.created_at_ms,
                claim: token,
            });
        }
        Ok(claimed)
    }

    async fn complete(&self, message: &OutboxMessage) -> Result<bool, StoreError> {
        let mut inner = self.lock();
        let before = inner.rows.len();
        inner.rows.retain(|row| !holds(row, message));
        Ok(inner.rows.len() < before)
    }

    async fn release(&self, message: &OutboxMessage, error: &str, retry_at_ms: i64) -> Result<bool, StoreError> {
        let mut inner = self.lock();
        match inner.rows.iter_mut().find(|row| holds(row, message)) {
            Some(row) => {
                row.attempts += 1;
                row.available_at_ms = retry_at_ms;
                row.claimed_until_ms = None;
                row.claim = None;
                row.last_error = Some(error.to_owned());
                Ok(true)
            }
            None => Ok(false),
        }
    }

    async fn bury(&self, message: &OutboxMessage, error: &str) -> Result<bool, StoreError> {
        let mut inner = self.lock();
        match inner.rows.iter_mut().find(|row| holds(row, message)) {
            Some(row) => {
                row.attempts += 1;
                row.status = Status::Dead;
                row.claimed_until_ms = None;
                row.claim = None;
                row.last_error = Some(error.to_owned());
                Ok(true)
            }
            None => Ok(false),
        }
    }

    async fn pending_len(&self) -> Result<usize, StoreError> {
        Ok(self
            .lock()
            .rows
            .iter()
            .filter(|row| row.status == Status::Pending)
            .count())
    }

    async fn dead_len(&self) -> Result<usize, StoreError> {
        Ok(self.lock().rows.iter().filter(|row| row.status == Status::Dead).count())
    }

    async fn requeue_dead(&self, id: &str) -> Result<bool, StoreError> {
        let mut inner = self.lock();
        match inner
            .rows
            .iter_mut()
            .find(|row| row.id == id && row.status == Status::Dead)
        {
            Some(row) => {
                row.status = Status::Pending;
                row.attempts = 0;
                row.available_at_ms = 0;
                Ok(true)
            }
            None => Ok(false),
        }
    }
}
