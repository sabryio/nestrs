use nestrs::prelude::*;

use crate::infrastructure::shared_state::{
    RateLimitStorePort, RedisHealth, SharedRedis, idempotency_store, rate_limit_store, shared_redis,
};
use crate::presentation::http::IdempotentWrites;

/// State that must be shared when the service runs as several instances: idempotency keys and
/// the global rate limit counters. Exports the idempotency interceptor (for controllers) and
/// the rate limit store (for `main`); the Redis connection and the idempotency store stay private.
#[module(
    providers = [shared_redis, idempotency_store, rate_limit_store, IdempotentWrites, RedisHealth],
    exports = [IdempotentWrites, RateLimitStorePort, SharedRedis],
)]
pub struct SharedStateModule;
