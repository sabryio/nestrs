#![allow(missing_docs)]
//! End to end: a real socket, the whole fx graph, and a graceful stop.

use std::net::SocketAddr;
use std::sync::Arc;

use fx_axum::domain::{Clock, ClockPort, GreetError, Greeting, GreetingService, GreetingStore};
use fx_axum::web::server::{Server, ServerConfig};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

fn any_port() -> ServerConfig {
    ServerConfig {
        addr: SocketAddr::from(([127, 0, 0, 1], 0)),
    }
}

/// One HTTP/1.1 request over a plain socket; returns (status, body).
async fn call(addr: SocketAddr, method: &str, path: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let request = format!("{method} {path} HTTP/1.1\r\nHost: test\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).await.unwrap();
    let status = raw.split_whitespace().nth(1).unwrap().parse().unwrap();
    let body = raw.split("\r\n\r\n").nth(1).unwrap_or_default().to_owned();
    (status, body)
}

struct Fixed;
impl Clock for Fixed {
    fn now(&self) -> u64 {
        42
    }
}

#[tokio::test]
async fn serves_requests_and_stops_gracefully() {
    let mut app = fx_axum::app(any_port()).replace::<ClockPort>(Arc::new(Fixed));
    app.start().await.unwrap();
    let addr = app.get::<Server>().await.unwrap().addr();

    assert_eq!(call(addr, "GET", "/health").await, (200, "ok".into()));

    let (status, body) = call(addr, "POST", "/greetings/Ada").await;
    assert_eq!(status, 201);
    assert!(body.contains("Hello, Ada!") && body.contains("\"at\":42"), "{body}");

    let (status, body) = call(addr, "GET", "/greetings").await;
    assert_eq!(status, 200);
    assert!(body.contains("Ada"), "{body}");

    assert_eq!(call(addr, "POST", "/greetings/%20").await.0, 422);

    app.stop().await.unwrap();
    assert!(
        TcpStream::connect(addr).await.is_err(),
        "the port is released after stop"
    );
}

#[tokio::test]
async fn a_busy_port_fails_the_start_with_a_clear_error() {
    let mut first = fx_axum::app(any_port());
    first.start().await.unwrap();
    let taken = first.get::<Server>().await.unwrap().addr();

    let mut second = fx_axum::app(ServerConfig { addr: taken });
    let error = second.start().await.unwrap_err();
    assert!(error.to_string().to_lowercase().contains("in use"), "{error}");
    first.stop().await.unwrap();
}

#[test]
fn the_wiring_validates_without_binding_anything() {
    fx_axum::app(any_port()).validate().unwrap();
}

// ---- the domain on its own: no fx, no axum, hand-written fakes ----------------------

#[derive(Default)]
struct Memory(std::sync::Mutex<Vec<Greeting>>);
impl GreetingStore for Memory {
    fn save(&self, greeting: Greeting) {
        self.0.lock().unwrap().push(greeting);
    }
    fn all(&self) -> Vec<Greeting> {
        self.0.lock().unwrap().clone()
    }
}

#[test]
fn the_service_is_testable_without_any_framework() {
    let service = GreetingService::new(Arc::new(Fixed), Arc::new(Memory::default()));
    assert_eq!(service.greet("  Grace ").unwrap().name, "Grace");
    assert_eq!(service.greet("   "), Err(GreetError::EmptyName));
    assert_eq!(service.history().len(), 1);
}
