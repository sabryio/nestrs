//! A real `redis-server` on a free port, killed when dropped.

use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use nestrs_redis::RedisConnection;

pub struct Server {
    pub child: Child,
    pub port: u16,
}

/// With `REQUIRE_REDIS` set (CI), a missing `redis-server` fails the run instead of skipping it.
fn missing(error: &std::io::Error) {
    assert!(
        std::env::var_os("REQUIRE_REDIS").is_none(),
        "REQUIRE_REDIS is set but redis-server cannot be started: {error}"
    );
}

impl Server {
    pub fn start() -> Option<Self> {
        let port = TcpListener::bind("127.0.0.1:0").ok()?.local_addr().ok()?.port();
        Self::start_on(port)
    }

    pub fn start_on(port: u16) -> Option<Self> {
        let child = Command::new("redis-server")
            .args([
                "--port",
                &port.to_string(),
                "--save",
                "",
                "--appendonly",
                "no",
                "--bind",
                "127.0.0.1",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .inspect_err(missing)
            .ok()?;
        Some(Self { child, port })
    }

    pub async fn connect(&self) -> RedisConnection {
        let url = format!("redis://127.0.0.1:{}/", self.port);
        for _ in 0..50 {
            if let Ok(connection) = RedisConnection::connect(&url).await {
                return connection;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("redis-server did not come up");
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
