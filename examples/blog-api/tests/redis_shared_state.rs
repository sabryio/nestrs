//! Two instances of the app, one Redis: a retry that lands on the other instance
//! replays the first response instead of running the write again. Skipped when
//! `redis-server` is not installed.

use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use blog_api::AppModule;
use blog_api::infrastructure::config::AppConfig;
use nestrs::http::axum::http::StatusCode;
use nestrs_testing::{TestApp, TestClient};
use serde_json::{Value, json};

struct Redis(Child);

impl Drop for Redis {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// With `REQUIRE_REDIS` set (CI), a missing `redis-server` fails the run instead of skipping it.
fn missing(error: &std::io::Error) {
    assert!(
        std::env::var_os("REQUIRE_REDIS").is_none(),
        "REQUIRE_REDIS is set but redis-server cannot be started: {error}"
    );
}

fn start_redis() -> Option<(Redis, String)> {
    let port = TcpListener::bind("127.0.0.1:0").ok()?.local_addr().ok()?.port();
    let child = Command::new("redis-server")
        .args(["--port", &port.to_string(), "--save", "", "--bind", "127.0.0.1"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .inspect_err(missing)
        .ok()?;
    Some((Redis(child), format!("redis://127.0.0.1:{port}/")))
}

async fn instance(url: &str) -> TestApp {
    let config = AppConfig {
        redis_url: Some(url.to_owned()),
        ..AppConfig::default()
    };
    // The server needs a moment to accept connections.
    for _ in 0..50 {
        let app = TestApp::builder(AppModule)
            .override_value(config.clone())
            .try_start()
            .await;
        if let Ok(app) = app {
            return app;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("could not start the app against Redis");
}

async fn signup(client: &TestClient) -> nestrs_testing::TestResponse {
    client
        .post("/users")
        .bearer("dev-token")
        .header("idempotency-key", "signup-42")
        .json(&json!({ "name": "Ada", "email": "ada@example.com" }))
        .send()
        .await
}

#[tokio::test]
async fn a_retry_on_another_instance_replays_the_first_response() {
    let Some((_redis, url)) = start_redis() else {
        eprintln!("redis-server not installed: skipping");
        return;
    };
    let (one, two) = (instance(&url).await, instance(&url).await);

    let first = signup(&one.client()).await.assert_status(StatusCode::CREATED);
    let retry = signup(&two.client()).await.assert_status(StatusCode::CREATED);

    assert_eq!(retry.header("idempotent-replayed"), Some("true"));
    assert_eq!(first.json::<Value>()["id"], retry.json::<Value>()["id"]);
    let users: Value = two.client().get("/users").send().await.json();
    assert_eq!(users, json!([]), "the second instance never ran the write");
}

#[tokio::test]
async fn readiness_reports_redis_when_it_is_configured() {
    let Some((_redis, url)) = start_redis() else {
        eprintln!("redis-server not installed: skipping");
        return;
    };
    let app = instance(&url).await;
    let ready: Value = app
        .client()
        .get("/health/ready")
        .send()
        .await
        .assert_status(StatusCode::OK)
        .json();
    let redis = ready["checks"]
        .as_array()
        .and_then(|checks| checks.iter().find(|check| check["name"] == "redis"))
        .unwrap_or_else(|| panic!("redis is configured, so it is reported: {ready}"));
    assert_eq!(redis["status"], "up");
}
