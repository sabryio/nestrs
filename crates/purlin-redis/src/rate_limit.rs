use std::time::Duration;

use purlin_core::BoxFuture;
use purlin_http::{Decision, RateLimitStore, StoreError};
use redis::Script;

use crate::RedisConnection;

/// GCRA, the same algorithm as the in-memory store, decided inside Redis so the check
/// and the update are one atomic step. Times are microseconds from the *server's* clock
/// (`TIME`), so instances with skewed clocks still agree. Returns
/// `{allowed, remaining, retry_after_us, reset_after_us}`.
const GCRA: &str = r"
local t = redis.call('TIME')
local now = t[1] * 1000000 + t[2]
local limit = tonumber(ARGV[1])
local window = tonumber(ARGV[2])
local interval = math.max(math.floor(window / limit), 1)
local stored = tonumber(redis.call('GET', KEYS[1]))
local tat = now
if stored and stored > now then tat = stored end
local new_tat = tat + interval
local edge = now + window
if new_tat <= edge then
  local remaining = math.floor((edge - new_tat) / interval)
  redis.call('SET', KEYS[1], string.format('%d', new_tat), 'PX', math.ceil((new_tat - now) / 1000) + 1)
  return {1, remaining, 0, new_tat - now}
end
return {0, 0, new_tat - edge, math.max(tat - now, 0)}
";

/// A [`RateLimitStore`] in Redis: the limit holds across every instance of the service.
///
/// Each client costs one small key that expires on its own when the client goes quiet.
#[derive(Debug, Clone)]
pub struct RedisRateLimitStore {
    redis: RedisConnection,
    prefix: String,
}

impl RedisRateLimitStore {
    /// Stores counters under the `purlin` key prefix.
    pub fn new(redis: RedisConnection) -> Self {
        Self {
            redis,
            prefix: "purlin".to_owned(),
        }
    }

    /// Uses `prefix` instead of `purlin`, so several apps can share one Redis.
    pub fn prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }
}

impl RateLimitStore for RedisRateLimitStore {
    fn hit<'a>(&'a self, key: &'a str, limit: u32, window: Duration) -> BoxFuture<'a, Result<Decision, StoreError>> {
        Box::pin(async move {
            let mut connection = self.redis.handle();
            let reply: Vec<i64> = Script::new(GCRA)
                .key(format!("{}:rl:{key}", self.prefix))
                .arg(limit.max(1))
                .arg(window.as_micros().max(1) as u64)
                .invoke_async(&mut connection)
                .await
                .map_err(|error| StoreError(error.to_string()))?;
            let [allowed, remaining, retry_us, reset_us] = reply[..] else {
                return Err(StoreError(format!(
                    "unexpected reply from the rate limit script: {reply:?}"
                )));
            };
            let micros = |value: i64| Duration::from_micros(value.max(0) as u64);
            Ok(Decision {
                allowed: allowed == 1,
                remaining: u32::try_from(remaining.max(0)).unwrap_or(u32::MAX),
                retry_after: micros(retry_us),
                reset_after: micros(reset_us),
            })
        })
    }
}
