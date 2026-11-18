use std::time::Duration;

use redis::aio::{ConnectionManager, ConnectionManagerConfig};

pub use redis::RedisError;

/// A shared, auto-reconnecting handle to Redis. Cloning is cheap; every clone uses the
/// same underlying connection.
///
/// Commands give up after a short timeout (500 ms by default) so a dead Redis slows
/// requests down briefly instead of hanging them.
#[derive(Clone)]
pub struct RedisConnection {
    client: redis::Client,
    manager: ConnectionManager,
}

impl RedisConnection {
    /// Connects to `url` (`redis://host:port/db`, `rediss://` for TLS).
    pub async fn connect(url: &str) -> Result<Self, RedisError> {
        Self::connect_with_timeout(url, Duration::from_millis(500)).await
    }

    /// Like [`connect`](Self::connect) with your own per-command timeout.
    pub async fn connect_with_timeout(url: &str, command_timeout: Duration) -> Result<Self, RedisError> {
        let client = redis::Client::open(url)?;
        let config = ConnectionManagerConfig::new()
            .set_connection_timeout(Duration::from_secs(2))
            .set_response_timeout(command_timeout)
            .set_number_of_retries(1);
        let manager = ConnectionManager::new_with_config(client.clone(), config).await?;
        Ok(Self { client, manager })
    }

    /// Sends `PING`; `Ok` means Redis answered within the command timeout.
    pub async fn ping(&self) -> Result<(), RedisError> {
        redis::cmd("PING")
            .query_async::<String>(&mut self.handle())
            .await
            .map(drop)
    }

    /// A connection of its own, for blocking commands (`XREAD BLOCK`): on the shared multiplexed
    /// connection a blocking command would stall everyone else's.
    pub(crate) async fn dedicated(&self) -> Result<redis::aio::MultiplexedConnection, RedisError> {
        self.client.get_multiplexed_async_connection().await
    }

    /// A fresh handle for one command (the manager is multiplexed, so this is free).
    pub(crate) fn handle(&self) -> ConnectionManager {
        self.manager.clone()
    }
}

impl std::fmt::Debug for RedisConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RedisConnection")
    }
}
