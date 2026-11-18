use std::time::Duration;

use purlin_core::BoxFuture;
use purlin_interceptors::{CacheStore, Snapshot};

use crate::RedisConnection;
use crate::codec::{decode, encode};

/// A [`CacheStore`] in Redis: one response cache shared by every instance.
///
/// Entries expire in Redis itself (`PX`), so there is no sweeping and memory is bounded
/// by your Redis `maxmemory` policy. Per the port's contract it is best effort: a Redis
/// error is logged and acts as a miss, and an entry that cannot be decoded (a stale
/// format after an upgrade, corruption) is treated as absent.
#[derive(Debug, Clone)]
pub struct RedisCacheStore {
    redis: RedisConnection,
    prefix: String,
}

impl RedisCacheStore {
    /// Caches under the `purlin` key prefix.
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

    fn key(&self, key: &str) -> String {
        format!("{}:cache:{key}", self.prefix)
    }
}

impl CacheStore for RedisCacheStore {
    fn get<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Option<Snapshot>> {
        Box::pin(async move {
            let mut connection = self.redis.handle();
            let stored: Result<Option<Vec<u8>>, _> =
                redis::cmd("GET").arg(self.key(key)).query_async(&mut connection).await;
            match stored {
                Ok(bytes) => bytes.and_then(|bytes| decode(&bytes)),
                Err(error) => {
                    tracing::warn!(%error, "cache read failed; treating it as a miss");
                    None
                }
            }
        })
    }

    fn put<'a>(&'a self, key: String, value: Snapshot, ttl: Duration) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let mut connection = self.redis.handle();
            let stored: Result<(), _> = redis::cmd("SET")
                .arg(self.key(&key))
                .arg(encode(&value))
                .arg("PX")
                .arg(ttl.as_millis().max(1) as u64)
                .query_async(&mut connection)
                .await;
            if let Err(error) = stored {
                tracing::warn!(%error, "cache write failed; the response was not cached");
            }
        })
    }
}
