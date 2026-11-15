//! A [`ValueStore`] in Redis.

use std::time::Duration;

use nestrs_cache::{StoreError, ValueStore};
use redis::AsyncCommands;

use crate::RedisConnection;

/// Service-level cache values in Redis, shared by every instance: `SET key value PX ttl`.
#[derive(Debug, Clone)]
pub struct RedisValueStore {
    redis: RedisConnection,
    prefix: String,
}

impl RedisValueStore {
    /// Keys live under `nestrs:v:`.
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

    fn key(&self, key: &str) -> String {
        format!("{}:v:{key}", self.prefix)
    }
}

fn error(error: redis::RedisError) -> StoreError {
    StoreError(error.to_string())
}

#[async_trait::async_trait]
impl ValueStore for RedisValueStore {
    async fn get(&self, key: &str) -> Result<Option<String>, StoreError> {
        self.redis.handle().get(self.key(key)).await.map_err(error)
    }

    async fn put(&self, key: &str, value: String, ttl: Duration) -> Result<(), StoreError> {
        let millis = u64::try_from(ttl.as_millis()).unwrap_or(u64::MAX).max(1);
        self.redis
            .handle()
            .pset_ex::<_, _, ()>(self.key(key), value, millis)
            .await
            .map_err(error)
    }

    async fn remove(&self, key: &str) -> Result<(), StoreError> {
        self.redis.handle().del::<_, ()>(self.key(key)).await.map_err(error)
    }
}
