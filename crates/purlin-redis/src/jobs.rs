//! Redis adapters for `purlin-jobs`: the tick lock and the queue backend.

use std::time::Duration;

use purlin_jobs::{Envelope, JobLock, LockError, QueueBackend, QueueError, Reserved};
use redis::{AsyncCommands, Script};

use crate::RedisConnection;

/// A [`JobLock`] in Redis: `SET key NX PX ttl`, so exactly one instance wins each window.
#[derive(Debug, Clone)]
pub struct RedisJobLock {
    redis: RedisConnection,
    prefix: String,
}

impl RedisJobLock {
    /// Keys live under `purlin:lock:`.
    pub fn new(redis: RedisConnection) -> Self {
        Self {
            redis,
            prefix: "purlin".to_owned(),
        }
    }

    /// Uses `prefix` instead of `purlin`.
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

/// Takes the oldest waiting item and leases it, in one step, using the server's clock.
const RESERVE: &str = r"
local item = redis.call('LPOP', KEYS[1])
if not item then return false end
local t = redis.call('TIME')
local now = t[1] * 1000 + math.floor(t[2] / 1000)
redis.call('ZADD', KEYS[2], now + tonumber(ARGV[1]), item)
return item
";

/// Extends a lease that is still held.
const RENEW: &str = r"
if not redis.call('ZSCORE', KEYS[1], ARGV[1]) then return 0 end
local t = redis.call('TIME')
local now = t[1] * 1000 + math.floor(t[2] / 1000)
redis.call('ZADD', KEYS[1], 'XX', now + tonumber(ARGV[2]), ARGV[1])
return 1
";

/// Ends a held lease and puts the replacement on the queue. Does nothing if the lease was lost.
const RETRY: &str = r"
if redis.call('ZREM', KEYS[1], ARGV[1]) == 1 then
  redis.call('RPUSH', KEYS[2], ARGV[2])
  return 1
end
return 0
";

/// Ends a held lease and moves the item to the dead letters. Does nothing if the lease was lost.
const BURY: &str = r"
if redis.call('ZREM', KEYS[1], ARGV[1]) == 1 then
  redis.call('RPUSH', KEYS[2], ARGV[1])
  return 1
end
return 0
";

/// Returns expired leases to the queue with one more attempt, or buries them at the limit.
const REAP: &str = r"
local t = redis.call('TIME')
local now = t[1] * 1000 + math.floor(t[2] / 1000)
local expired = redis.call('ZRANGEBYSCORE', KEYS[1], '-inf', now, 'LIMIT', 0, 100)
for _, raw in ipairs(expired) do
  redis.call('ZREM', KEYS[1], raw)
  local envelope = cjson.decode(raw)
  envelope.attempts = envelope.attempts + 1
  local out = cjson.encode(envelope)
  if envelope.attempts >= tonumber(ARGV[1]) then
    redis.call('RPUSH', KEYS[3], out)
  else
    redis.call('RPUSH', KEYS[2], out)
  end
end
return #expired
";

/// How long an empty `reserve` sleeps between looks. Atomic lease-taking needs a script,
/// and a script cannot block, so waiting is polling; this bounds the extra latency.
const POLL_STEP: Duration = Duration::from_millis(50);

/// A [`QueueBackend`] over Redis, with leases.
///
/// * `…:q:<name>` is the waiting list, `…:q:<name>:leases` a sorted set of leased items
///   scored by the lease's end (server clock), `…:q:<name>:dead` the buried.
/// * Taking an item and leasing it is one script, so there is no moment where a crash
///   leaves work neither waiting nor leased.
/// * `ack`, `retry`, `bury` and `renew` all test the lease first (`ZREM`/`ZSCORE`): a
///   worker that lost its lease cannot duplicate or erase work that moved on.
/// * The receipt is the stored JSON; every reap and retry raises `attempts`, so a new
///   holder's receipt never equals a previous holder's.
/// * `reserve` polls every 50 ms while the queue is empty, so a push is picked up within
///   about that long.
pub struct RedisQueueBackend {
    redis: RedisConnection,
    prefix: String,
}

impl RedisQueueBackend {
    /// Keys live under `purlin:q:`.
    pub fn new(redis: RedisConnection) -> Self {
        Self {
            redis,
            prefix: "purlin".to_owned(),
        }
    }

    /// Uses `prefix` instead of `purlin`.
    pub fn prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }

    fn waiting(&self, queue: &str) -> String {
        format!("{}:q:{queue}", self.prefix)
    }

    fn leases(&self, queue: &str) -> String {
        format!("{}:q:{queue}:leases", self.prefix)
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

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX).max(1)
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

    async fn reserve(&self, queue: &str, wait: Duration, lease: Duration) -> Result<Option<Reserved>, QueueError> {
        let deadline = tokio::time::Instant::now() + wait;
        let script = Script::new(RESERVE);
        loop {
            let mut connection = self.redis.handle();
            let raw: Option<String> = script
                .key(self.waiting(queue))
                .key(self.leases(queue))
                .arg(millis(lease))
                .invoke_async(&mut connection)
                .await
                .map_err(backend)?;
            if let Some(raw) = raw {
                let envelope = serde_json::from_str(&raw)
                    .map_err(|error| QueueError::Backend(format!("corrupt entry: {error}")))?;
                return Ok(Some(Reserved { envelope, receipt: raw }));
            }
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            if left.is_zero() {
                return Ok(None);
            }
            tokio::time::sleep(left.min(POLL_STEP)).await;
        }
    }

    async fn renew(&self, queue: &str, reserved: &Reserved, lease: Duration) -> Result<bool, QueueError> {
        let mut connection = self.redis.handle();
        let held: i64 = Script::new(RENEW)
            .key(self.leases(queue))
            .arg(&reserved.receipt)
            .arg(millis(lease))
            .invoke_async(&mut connection)
            .await
            .map_err(backend)?;
        Ok(held == 1)
    }

    async fn ack(&self, queue: &str, reserved: &Reserved) -> Result<(), QueueError> {
        let mut connection = self.redis.handle();
        connection
            .zrem::<_, _, ()>(self.leases(queue), &reserved.receipt)
            .await
            .map_err(backend)
    }

    async fn retry(&self, queue: &str, reserved: &Reserved, envelope: Envelope) -> Result<(), QueueError> {
        let replacement = encode(&envelope)?;
        let mut connection = self.redis.handle();
        Script::new(RETRY)
            .key(self.leases(queue))
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
            .key(self.leases(queue))
            .key(self.dead(queue))
            .arg(&reserved.receipt)
            .invoke_async::<i64>(&mut connection)
            .await
            .map(drop)
            .map_err(backend)
    }

    async fn reap(&self, queue: &str, max_attempts: u32) -> Result<usize, QueueError> {
        let mut connection = self.redis.handle();
        let mut total = 0;
        // The script takes at most 100 at a time; keep going until nothing is left.
        loop {
            let reaped: usize = Script::new(REAP)
                .key(self.leases(queue))
                .key(self.waiting(queue))
                .key(self.dead(queue))
                .arg(max_attempts)
                .invoke_async(&mut connection)
                .await
                .map_err(backend)?;
            total += reaped;
            if reaped < 100 {
                return Ok(total);
            }
        }
    }

    async fn len(&self, queue: &str) -> Result<usize, QueueError> {
        let mut connection = self.redis.handle();
        connection.llen(self.waiting(queue)).await.map_err(backend)
    }

    async fn leased_len(&self, queue: &str) -> Result<usize, QueueError> {
        let mut connection = self.redis.handle();
        connection.zcard(self.leases(queue)).await.map_err(backend)
    }

    async fn dead_len(&self, queue: &str) -> Result<usize, QueueError> {
        let mut connection = self.redis.handle();
        connection.llen(self.dead(queue)).await.map_err(backend)
    }
}
