//! State that must be shared when the service runs as several instances: idempotency
//! keys and the global rate limit. Each lives in process memory, or in Redis when
//! `BLOG_REDIS_URL` is set; the rest of the app sees only the ports.

use std::sync::Arc;

use purlin::http::{MemoryStore, RateLimitStore};
use purlin::prelude::*;
use purlin_health::{HealthIndicator, HealthIndicatorPort, Unhealthy};
use purlin_interceptors::{IdempotencyStorePort, MemoryIdempotencyStore};
use purlin_redis::{RedisConnection, RedisIdempotencyStore, RedisIndicator, RedisRateLimitStore};

use super::config::AppConfig;

/// The rate limit store as the container holds it.
pub type RateLimitStorePort = Arc<dyn RateLimitStore>;

/// The one Redis connection the shared-state adapters use, when configured.
pub struct SharedRedis(Option<RedisConnection>);

impl SharedRedis {
    /// The connection, when Redis is configured.
    pub fn connection(&self) -> Option<&RedisConnection> {
        self.0.as_ref()
    }
}

/// Connects to Redis when `AppConfig::redis_url` is set. A configured but unreachable Redis
/// fails startup: running a multi-instance service on silently local state would be worse.
#[provider]
pub async fn shared_redis(config: Arc<AppConfig>) -> Result<SharedRedis, DiError> {
    match &config.redis_url {
        None => Ok(SharedRedis(None)),
        Some(url) => RedisConnection::connect(url)
            .await
            .map(|connection| SharedRedis(Some(connection)))
            .map_err(|error| DiError::custom(format!("cannot connect to Redis: {error}"))),
    }
}

/// Idempotency keys: Redis when configured, else process memory (exact for one instance).
#[provider]
pub fn idempotency_store(redis: Arc<SharedRedis>) -> IdempotencyStorePort {
    match &redis.0 {
        Some(connection) => Arc::new(RedisIdempotencyStore::new(connection.clone())),
        None => Arc::new(MemoryIdempotencyStore::default()),
    }
}

/// Rate limit counters: Redis when configured (one limit across all instances), else memory.
#[provider]
pub fn rate_limit_store(redis: Arc<SharedRedis>) -> RateLimitStorePort {
    match &redis.0 {
        Some(connection) => Arc::new(RedisRateLimitStore::new(connection.clone())),
        None => Arc::new(MemoryStore::default()),
    }
}

/// Readiness for Redis. Non-critical: with Redis down the limiter fails open and idempotent
/// writes answer 503, which is worth reporting but not worth pulling the pod out of rotation.
/// Left out of the report entirely when Redis is not configured.
#[injectable(contributes = HealthIndicatorPort)]
pub struct RedisHealth {
    redis: Arc<SharedRedis>,
}

#[async_trait]
impl HealthIndicator for RedisHealth {
    fn name(&self) -> &str {
        "redis"
    }

    fn enabled(&self) -> bool {
        self.redis.connection().is_some()
    }

    fn critical(&self) -> bool {
        false
    }

    async fn check(&self) -> Result<(), Unhealthy> {
        match self.redis.connection() {
            Some(connection) => RedisIndicator::new(connection.clone()).check().await,
            None => Ok(()),
        }
    }
}
