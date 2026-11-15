//! Redis adapters for `nestrs-jobs`: the tick lock and the queue backend.

use std::time::Duration;

use nestrs_jobs::{Envelope, JobLock, LockError, QueueBackend, QueueError, Reserved};
use redis::aio::MultiplexedConnection;
use redis::{AsyncCommands, Script};
use tokio::sync::Mutex;

use crate::RedisConnection;

/// A [`JobLock`] in Redis: `SET key NX PX ttl`, so exactly one instance wins each window.
#[derive(Debug, Clone)]
pub struct RedisJobLock {
    redis: RedisConnection,
    prefix: String,
}

impl RedisJobLock {
    /// Keys live under `nestrs:lock:`.
    pub fn new(redis: RedisConnection) -> Self {
        Self {
            redis,
            prefix: "nestrs".to_owned(),
        }
    }

    /// Uses `prefix` instead of `nestrs`.
    pub fn prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }
}

#[async_trait::async_trait]
impl JobLock for RedisJobLock {
    async fn try_acquire(&self, name: &str, ttl: Duration) -> Result<bool, LockError> {
        let mut connection = self.redis.handle();
        let reply: Option<String> = redis::cmd("SET")
            .arg(format!("{}:lock:{name}", self.prefix))
            .arg(1)
            .arg("NX")
            .arg("PX")
            .arg(u64::try_from(ttl.as_millis()).unwrap_or(u64::MAX).max(1))
            .query_async(&mut connection)
            .await
            .map_err(|error| LockError(error.to_string()))?;
        Ok(reply.is_some())
    }
}

/// Removes the reserved copy and puts the replacement back, atomically.
const RETRY: &str = r"
redis.call('LREM', KEYS[1], 1, ARGV[1])
redis.call('RPUSH', KEYS[2], ARGV[2])
return 1
";

/// Removes the reserved copy and moves it to the dead letters, atomically.
const BURY: &str = r"
if redis.call('LREM', KEYS[1], 1, ARGV[1]) > 0 then
  redis.call('RPUSH', KEYS[2], ARGV[1])
end
return 1
";

/// A [`QueueBackend`] over Redis lists.
///
/// * `…:q:<name>` waits, `…:q:<name>:processing` holds reserved work, `…:q:<name>:dead` the buried.
/// * `reserve` is `BLMOVE waiting processing`: taking and hiding the work is one atomic step,
///   so a crashed worker leaves its work in `processing`, never nowhere.
/// * Blocking waits use connections of their own (pooled here), so they never stall the
///   shared connection other adapters use.
///
/// **Recovery caveat:** [`recover`](QueueBackend::recover) moves *everything* in `processing`
/// back, because Redis cannot tell a dead worker's work from a live one's. Run it only
/// where that is acceptable (delivery is at-least-once, so a rare duplicate is allowed).
pub struct RedisQueueBackend {
    redis: RedisConnection,
    prefix: String,
    idle: Mutex<Vec<MultiplexedConnection>>,
}

impl RedisQueueBackend {
    /// Keys live under `nestrs:q:`.
    pub fn new(redis: RedisConnection) -> Self {
        Self {
            redis,
            prefix: "nestrs".to_owned(),
            idle: Mutex::new(Vec::new()),
        }
    }

    /// Uses `prefix` instead of `nestrs`.
    pub fn prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }

    fn waiting(&self, queue: &str) -> String {
        format!("{}:q:{queue}", self.prefix)
    }

    fn processing(&self, queue: &str) -> String {
        format!("{}:q:{queue}:processing", self.prefix)
    }

    fn dead(&self, queue: &str) -> String {
        format!("{}:q:{queue}:dead", self.prefix)
    }
}

fn backend(error: redis::RedisError) -> QueueError {
    QueueError::Backend(error.to_string())
}

fn encode(envelope: &Envelope) -> Result<String, QueueError> {
    serde_json::to_string(envelope).map_err(|error| QueueError::Encode(error.to_string()))
}

#[async_trait::async_trait]
impl QueueBackend for RedisQueueBackend {
    async fn push(&self, queue: &str, envelope: Envelope) -> Result<(), QueueError> {
        let raw = encode(&envelope)?;
        let mut connection = self.redis.handle();
        connection
            .rpush::<_, _, ()>(self.waiting(queue), raw)
            .await
            .map_err(backend)
    }

    async fn reserve(&self, queue: &str, wait: Duration) -> Result<Option<Reserved>, QueueError> {
        let pooled = self.idle.lock().await.pop();
        let mut connection = match pooled {
            Some(connection) => connection,
            None => self.redis.dedicated().await.map_err(backend)?,
        };
        // No command timeout on this connection, so the server's own timeout ends the wait.
        let raw: Option<String> = redis::cmd("BLMOVE")
            .arg(self.waiting(queue))
            .arg(self.processing(queue))
            .arg("LEFT")
            .arg("RIGHT")
            .arg(wait.as_secs_f64().max(0.01))
            .query_async(&mut connection)
            .await
            .map_err(backend)?;
        // Only a healthy connection goes back to the pool.
        self.idle.lock().await.push(connection);
        let Some(raw) = raw else { return Ok(None) };
        let envelope =
            serde_json::from_str(&raw).map_err(|error| QueueError::Backend(format!("corrupt entry: {error}")))?;
        Ok(Some(Reserved { envelope, receipt: raw }))
    }

    async fn ack(&self, queue: &str, reserved: &Reserved) -> Result<(), QueueError> {
        let mut connection = self.redis.handle();
        connection
            .lrem::<_, _, ()>(self.processing(queue), 1, &reserved.receipt)
            .await
            .map_err(backend)
    }

    async fn retry(&self, queue: &str, reserved: &Reserved, envelope: Envelope) -> Result<(), QueueError> {
        let replacement = encode(&envelope)?;
        let mut connection = self.redis.handle();
        Script::new(RETRY)
            .key(self.processing(queue))
            .key(self.waiting(queue))
            .arg(&reserved.receipt)
            .arg(replacement)
            .invoke_async::<i64>(&mut connection)
            .await
            .map(drop)
            .map_err(backend)
    }

    async fn bury(&self, queue: &str, reserved: &Reserved) -> Result<(), QueueError> {
        let mut connection = self.redis.handle();
        Script::new(BURY)
            .key(self.processing(queue))
            .key(self.dead(queue))
            .arg(&reserved.receipt)
            .invoke_async::<i64>(&mut connection)
            .await
            .map(drop)
            .map_err(backend)
    }

    async fn recover(&self, queue: &str) -> Result<usize, QueueError> {
        let mut connection = self.redis.handle();
        let mut moved = 0;
        loop {
            let item: Option<String> = redis::cmd("LMOVE")
                .arg(self.processing(queue))
                .arg(self.waiting(queue))
                .arg("LEFT")
                .arg("RIGHT")
                .query_async(&mut connection)
                .await
                .map_err(backend)?;
            if item.is_none() {
                return Ok(moved);
            }
            moved += 1;
        }
    }

    async fn len(&self, queue: &str) -> Result<usize, QueueError> {
        let mut connection = self.redis.handle();
        connection.llen(self.waiting(queue)).await.map_err(backend)
    }

    async fn dead_len(&self, queue: &str) -> Result<usize, QueueError> {
        let mut connection = self.redis.handle();
        connection.llen(self.dead(queue)).await.map_err(backend)
    }
}
