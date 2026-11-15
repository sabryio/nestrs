use std::time::Duration;

use nestrs_core::BoxFuture;
use nestrs_interceptors::{Begin, IdempotencyStore, Snapshot};
use redis::Script;

use crate::RedisConnection;
use crate::codec::{decode, encode};

/// Claims a key atomically. Replies `{code, response}`:
/// 0 started, 1 replay (with the stored response), 2 in flight, 3 different request.
const BEGIN: &str = r"
if redis.call('EXISTS', KEYS[1]) == 0 then
  redis.call('HSET', KEYS[1], 'fp', ARGV[1])
  redis.call('PEXPIRE', KEYS[1], ARGV[2])
  return {0, false}
end
if redis.call('HGET', KEYS[1], 'fp') ~= ARGV[1] then return {3, false} end
local response = redis.call('HGET', KEYS[1], 'resp')
if response then return {1, response} end
return {2, false}
";

/// Stores the response, only if the claim still exists, and extends it to the full TTL.
const COMPLETE: &str = r"
if redis.call('EXISTS', KEYS[1]) == 1 then
  redis.call('HSET', KEYS[1], 'resp', ARGV[1])
  redis.call('PEXPIRE', KEYS[1], ARGV[2])
end
return 1
";

/// Releases a claim that never got a response; a finished key is left alone.
const ABANDON: &str = r"
if redis.call('HEXISTS', KEYS[1], 'resp') == 0 then redis.call('DEL', KEYS[1]) end
return 1
";

/// An [`IdempotencyStore`] in Redis: a retry that lands on a different instance (or
/// after a restart) still replays the original response.
///
/// A claim for a request that is *running* lives only `pending_ttl` (60 seconds by
/// default). If the instance handling it dies, the key frees itself and the client can
/// retry, instead of being locked out for the whole `ttl`. A *finished* key lives `ttl`
/// (24 hours by default).
///
/// If Redis cannot be reached, `begin` answers [`Begin::Unavailable`] and the
/// interceptor refuses the write with `503`.
#[derive(Debug, Clone)]
pub struct RedisIdempotencyStore {
    redis: RedisConnection,
    prefix: String,
    ttl: Duration,
    pending_ttl: Duration,
}

impl RedisIdempotencyStore {
    /// Remembers finished keys for 24 hours under the `nestrs` prefix.
    pub fn new(redis: RedisConnection) -> Self {
        Self {
            redis,
            prefix: "nestrs".to_owned(),
            ttl: Duration::from_secs(24 * 60 * 60),
            pending_ttl: Duration::from_secs(60),
        }
    }

    /// How long a finished key is remembered.
    pub fn ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
        self
    }

    /// How long a running request's claim lasts. Set it above your request timeout.
    pub fn pending_ttl(mut self, ttl: Duration) -> Self {
        self.pending_ttl = ttl;
        self
    }

    /// Uses `prefix` instead of `nestrs`.
    pub fn prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }

    fn key(&self, key: &str) -> String {
        format!("{}:idem:{key}", self.prefix)
    }
}

fn millis(duration: Duration) -> u64 {
    duration.as_millis().max(1) as u64
}

impl IdempotencyStore for RedisIdempotencyStore {
    fn begin<'a>(&'a self, key: &'a str, fingerprint: &'a str) -> BoxFuture<'a, Begin> {
        Box::pin(async move {
            let mut connection = self.redis.handle();
            let reply: Result<(i64, Option<Vec<u8>>), _> = Script::new(BEGIN)
                .key(self.key(key))
                .arg(fingerprint)
                .arg(millis(self.pending_ttl))
                .invoke_async(&mut connection)
                .await;
            match reply {
                Ok((0, _)) => Begin::Started,
                Ok((1, Some(bytes))) => match decode(&bytes) {
                    Some(snapshot) => Begin::Replay(snapshot),
                    None => {
                        tracing::error!("a stored idempotent response could not be decoded");
                        Begin::Unavailable
                    }
                },
                Ok((2, _)) => Begin::InFlight,
                Ok((3, _)) => Begin::Mismatch,
                Ok(other) => {
                    tracing::error!(?other, "unexpected reply from the idempotency script");
                    Begin::Unavailable
                }
                Err(error) => {
                    tracing::error!(%error, "idempotency store unreachable");
                    Begin::Unavailable
                }
            }
        })
    }

    fn complete<'a>(&'a self, key: &'a str, response: Snapshot) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let mut connection = self.redis.handle();
            let stored: Result<i64, _> = Script::new(COMPLETE)
                .key(self.key(key))
                .arg(encode(&response))
                .arg(millis(self.ttl))
                .invoke_async(&mut connection)
                .await;
            if let Err(error) = stored {
                tracing::error!(%error, "could not store the idempotent response; a retry will run the request again");
            }
        })
    }

    fn abandon<'a>(&'a self, key: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let mut connection = self.redis.handle();
            let released: Result<i64, _> = Script::new(ABANDON)
                .key(self.key(key))
                .invoke_async(&mut connection)
                .await;
            if let Err(error) = released {
                tracing::warn!(%error, "could not release an idempotency claim; it will expire on its own");
            }
        })
    }
}
