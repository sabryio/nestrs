//! # purlin-redis
//!
//! Redis implementations of purlin ports, so several instances of a service share state:
//!
//! | Adapter | Implements | Why Redis |
//! |---|---|---|
//! | [`RedisRateLimitStore`] | `purlin_http::RateLimitStore` | one limit across all instances |
//! | [`RedisCacheStore`] | `purlin_interceptors::CacheStore` | one response cache for all instances |
//! | [`RedisIdempotencyStore`] | `purlin_interceptors::IdempotencyStore` | a retry that lands on another instance still replays |
//! | [`RedisEventStream`] | a replayable event feed (Redis Streams) | SSE and other events reach clients on every instance, and resume by cursor |
//! | [`RedisValueStore`] | `purlin_cache::ValueStore` | service-level read-through cache shared by every instance |
//! | [`RedisJobLock`] | `purlin_jobs::JobLock` | one instance runs each scheduled tick |
//! | [`RedisQueueBackend`] | `purlin_jobs::QueueBackend` | durable, shared background queues with ack/retry/dead letters |
//! | [`RedisIndicator`] | `purlin_health::HealthIndicator` | report Redis in `/health/ready` (non-critical by default) |
//!
//! All three share one [`RedisConnection`], a cheap-to-clone handle that reconnects by
//! itself. Every multi-step decision is a single Lua script, so it is atomic on the
//! server: no two instances can both win a race.
//!
//! ```ignore
//! let redis = RedisConnection::connect("redis://127.0.0.1/").await?;
//! RateLimit::per_minute(60).store(Arc::new(RedisRateLimitStore::new(redis.clone())))
//! ResponseCache::new(Arc::new(RedisCacheStore::new(redis.clone())), Duration::from_secs(30))
//! Idempotency::new(Arc::new(RedisIdempotencyStore::new(redis)))
//! ```
//!
//! ## When Redis is down
//!
//! Each port already says what its adapter must do, and the adapters follow it:
//! the rate limiter reports a store error (the `RateLimit` interceptor then *fails open*),
//! the cache behaves as a miss, and idempotency returns `Begin::Unavailable` (the
//! interceptor then *fails closed* with `503`, because running a write unprotected could
//! repeat it). Calls time out quickly, so an outage costs milliseconds, not request threads.

#![forbid(unsafe_code)]

mod cache;
mod codec;
mod connection;
mod health;
mod idempotency;
mod jobs;
mod rate_limit;
mod stream;
mod value;

pub use cache::RedisCacheStore;
pub use connection::{RedisConnection, RedisError};
pub use health::RedisIndicator;
pub use idempotency::RedisIdempotencyStore;
pub use jobs::{RedisJobLock, RedisQueueBackend};
pub use rate_limit::RedisRateLimitStore;
pub use stream::{RedisEventStream, StreamEntry, StreamId, StreamSubscription};
pub use value::RedisValueStore;
