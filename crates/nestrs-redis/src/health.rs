use nestrs::prelude::async_trait;
use nestrs_health::{HealthIndicator, Unhealthy};

use crate::RedisConnection;

/// Readiness check for Redis: a `PING` within the connection's command timeout.
///
/// **Non-critical by default**: the stores in this crate degrade (the cache misses, the
/// rate limiter fails open) rather than stop the service, so a Redis outage should be
/// *reported* without pulling every instance out of rotation. Call
/// [`required`](Self::required) if your service cannot work without it, for example when
/// idempotency is mandatory on every write.
///
/// The failure message is fixed text: driver errors can name hosts.
#[derive(Debug, Clone)]
pub struct RedisIndicator {
    redis: RedisConnection,
    name: String,
    critical: bool,
}

impl RedisIndicator {
    /// A non-critical check named `"redis"`.
    pub fn new(redis: RedisConnection) -> Self {
        Self {
            redis,
            name: "redis".to_owned(),
            critical: false,
        }
    }

    /// Reports under another name, for services with several Redis instances.
    pub fn named(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Makes a failure mark the service not ready.
    pub fn required(mut self) -> Self {
        self.critical = true;
        self
    }
}

#[async_trait]
impl HealthIndicator for RedisIndicator {
    fn name(&self) -> &str {
        &self.name
    }

    fn critical(&self) -> bool {
        self.critical
    }

    async fn check(&self) -> Result<(), Unhealthy> {
        self.redis.ping().await.map_err(|error| {
            tracing::warn!(%error, "redis ping failed");
            Unhealthy::new("redis unreachable")
        })
    }
}
