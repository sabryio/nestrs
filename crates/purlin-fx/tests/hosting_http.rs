//! The HTTP transport as an fx component: bind at construction, serve on start, drain on stop.
#![cfg(feature = "http")]
#![allow(missing_docs)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use purlin::lifecycle;
use purlin::prelude::*;
use purlin_fx::{HttpServer, hosted, serve};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

static CLOSED: AtomicBool = AtomicBool::new(false);

#[injectable]
struct Db;

#[lifecycle]
impl Db {
    #[on_destroy]
    async fn close(&self) -> Result<(), String> {
        CLOSED.store(true, Ordering::SeqCst);
        Ok(())
    }
}

#[injectable]
struct Pings {
    _db: Arc<Db>,
}

#[controller("/ping")]
impl Pings {
    #[get]
    async fn ping(&self) -> &'static str {
        "pong"
    }
}

#[module(providers = [Db], controllers = [Pings])]
struct AppModule;

async fn get(addr: std::net::SocketAddr, path: &str) -> String {
    let mut stream = TcpStream::connect(addr).await.expect("connects");
    let request = format!("GET {path} HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut body = String::new();
    stream.read_to_string(&mut body).await.unwrap();
    body
}

#[tokio::test]
async fn controllers_are_served_while_the_app_runs_and_released_when_it_stops() {
    let mut app = fx::App::new()
        .options(hosted::<AppModule>())
        .options(serve("127.0.0.1:0"));
    app.start().await.expect("starts");

    let addr = app.get::<HttpServer>().await.expect("server is built").local_addr();
    let ok = get(addr, "/ping").await;
    assert!(ok.starts_with("HTTP/1.1 200"), "{ok}");
    assert!(ok.ends_with("pong"), "{ok}");
    assert!(get(addr, "/nope").await.starts_with("HTTP/1.1 404"));

    assert!(!CLOSED.load(Ordering::SeqCst));
    app.stop().await.expect("stops");
    assert!(
        CLOSED.load(Ordering::SeqCst),
        "the graph's destroy hooks ran exactly via fx"
    );
    assert!(TcpStream::connect(addr).await.is_err(), "the listener is closed");
}

#[tokio::test]
async fn an_unbindable_address_fails_the_start_phase() {
    let mut app = fx::App::new()
        .options(hosted::<AppModule>())
        .options(serve("256.0.0.1:1"));
    assert!(app.start().await.is_err());
}
