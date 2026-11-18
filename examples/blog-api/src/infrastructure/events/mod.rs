//! Adapters for the [`PostEvents`](crate::domain::PostEvents) port.

mod broadcast;
mod redis;

pub use broadcast::BroadcastPostEvents;
pub use redis::RedisPostEvents;

use std::sync::Arc;

use purlin::prelude::*;

use super::shared_state::SharedRedis;
use crate::domain::PostEventsPort;

/// The stream the blog's events live in, inside a shared Redis.
const STREAM_KEY: &str = "blog:post-events";

/// Picks the [`PostEventsPort`] adapter: the Redis Stream feed when `BLOG_REDIS_URL` is set (every
/// instance publishes into and serves from one ordered, resumable feed), else the in-process one.
#[provider]
pub async fn post_events(redis: Arc<SharedRedis>, local: Arc<BroadcastPostEvents>) -> Result<PostEventsPort, DiError> {
    match redis.connection() {
        None => Ok(local),
        Some(connection) => RedisPostEvents::connect(connection.clone(), STREAM_KEY)
            .await
            .map(|events| Arc::new(events) as PostEventsPort)
            .map_err(|error| DiError::custom(format!("cannot start the Redis event feed: {error}"))),
    }
}
