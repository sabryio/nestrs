//! Two instances of the app, one Redis: a post published on one instance reaches an SSE client
//! connected to the other, and a `Last-Event-ID` taken from one instance resumes on the other.
//! Skipped when `redis-server` is not installed.

use std::net::{SocketAddr, TcpListener as StdListener};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use blog_api::AppModule;
use blog_api::infrastructure::config::AppConfig;
use nestrs::http::axum;
use nestrs_testing::TestApp;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;

const FLUSH: &str = ": \n\n";

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
    let port = StdListener::bind("127.0.0.1:0").ok()?.local_addr().ok()?.port();
    let child = Command::new("redis-server")
        .args(["--port", &port.to_string(), "--save", "", "--bind", "127.0.0.1"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .inspect_err(missing)
        .ok()?;
    Some((Redis(child), format!("redis://127.0.0.1:{port}/")))
}

/// One app instance served on a real socket.
async fn instance(url: &str) -> (TestApp, SocketAddr) {
    let config = AppConfig {
        redis_url: Some(url.to_owned()),
        ..AppConfig::default()
    };
    let mut app = None;
    for _ in 0..50 {
        match TestApp::builder(AppModule)
            .override_value(config.clone())
            .try_start()
            .await
        {
            Ok(started) => {
                app = Some(started);
                break;
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
    let app = app.expect("could not start the app against Redis");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = app.app().router();
    tokio::spawn(async move { axum::serve(listener, router).await });
    (app, addr)
}

async fn post_json(addr: SocketAddr, path: &str, body: Value) -> Value {
    let mut socket = TcpStream::connect(addr).await.unwrap();
    let payload = body.to_string();
    let text = format!(
        "POST {path} HTTP/1.0\r\nauthorization: Bearer dev-token\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{payload}",
        payload.len()
    );
    socket.write_all(text.as_bytes()).await.unwrap();
    let mut raw = String::new();
    timeout(Duration::from_secs(5), socket.read_to_string(&mut raw))
        .await
        .unwrap()
        .unwrap();
    serde_json::from_str(raw.split_once("\r\n\r\n").unwrap().1).unwrap()
}

async fn publish(addr: SocketAddr, title: &str) -> Value {
    let email = format!("{}@example.com", title.to_lowercase());
    let author = post_json(addr, "/users", json!({ "name": "Ada", "email": email })).await;
    post_json(
        addr,
        "/posts",
        json!({ "author_id": author["id"], "title": title, "body": "b" }),
    )
    .await
}

async fn open_events(addr: SocketAddr, last_event_id: Option<&str>) -> (TcpStream, String) {
    let mut socket = TcpStream::connect(addr).await.unwrap();
    let resume = last_event_id
        .map(|id| format!("last-event-id: {id}\r\n"))
        .unwrap_or_default();
    socket
        .write_all(format!("GET /posts/events HTTP/1.0\r\naccept: text/event-stream\r\n{resume}\r\n").as_bytes())
        .await
        .unwrap();
    let mut seen = String::new();
    read_until(&mut socket, &mut seen, FLUSH).await;
    (socket, seen)
}

async fn read_until(socket: &mut TcpStream, seen: &mut String, needle: &str) {
    let mut buffer = [0u8; 4096];
    timeout(Duration::from_secs(5), async {
        while !seen.contains(needle) {
            let read = socket.read(&mut buffer).await.unwrap();
            assert!(read > 0, "stream ended before {needle:?}; got {seen:?}");
            seen.push_str(std::str::from_utf8(&buffer[..read]).unwrap());
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {needle:?}; got {seen:?}"));
}

/// The `id:` of the frame that carries `title`.
fn id_of(seen: &str, title: &str) -> String {
    let frame = seen
        .split("\n\n")
        .find(|frame| frame.contains(&format!("\"title\":\"{title}\"")))
        .unwrap_or_else(|| panic!("no frame for {title}: {seen:?}"));
    frame
        .lines()
        .find_map(|line| line.strip_prefix("id: "))
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn an_event_published_on_one_instance_reaches_a_client_on_the_other() {
    let Some((_redis, url)) = start_redis() else {
        eprintln!("redis-server not installed: skipping");
        return;
    };
    let ((_a, addr_a), (_b, addr_b)) = (instance(&url).await, instance(&url).await);

    let (mut client, mut seen) = open_events(addr_b, None).await;
    publish(addr_a, "Hello").await;

    read_until(&mut client, &mut seen, "\"title\":\"Hello\"").await;
}

#[tokio::test]
async fn last_event_id_from_one_instance_resumes_on_the_other() {
    let Some((_redis, url)) = start_redis() else {
        eprintln!("redis-server not installed: skipping");
        return;
    };
    let ((_a, addr_a), (_b, addr_b)) = (instance(&url).await, instance(&url).await);

    let (mut first, mut seen) = open_events(addr_a, None).await;
    publish(addr_a, "p1").await;
    read_until(&mut first, &mut seen, "\"title\":\"p1\"").await;
    let resume_from = id_of(&seen, "p1");
    drop(first);

    // Missed while disconnected, published through the other instance.
    publish(addr_b, "p2").await;
    publish(addr_a, "p3").await;

    let (mut second, mut seen) = open_events(addr_b, Some(&resume_from)).await;
    read_until(&mut second, &mut seen, "\"title\":\"p3\"").await;
    assert!(seen.contains("\"title\":\"p2\""), "{seen:?}");
    assert!(!seen.contains("\"title\":\"p1\""), "p1 was already seen: {seen:?}");
    assert!(
        seen.find("\"title\":\"p2\"") < seen.find("\"title\":\"p3\""),
        "replay keeps order: {seen:?}"
    );
}
