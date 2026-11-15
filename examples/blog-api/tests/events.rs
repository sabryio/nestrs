//! Typed SSE over a real socket, checked byte for byte: flush, then messages, then
//! close. The test speaks HTTP/1.0 on raw TCP so the response has no chunk framing
//! and what it reads is exactly what the server wrote.

use std::time::Duration;

use blog_api::AppModule;
use nestrs::http::axum;
use nestrs::openapi::ApiInfo;
use nestrs::prelude::App;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;

const FLUSH: &str = ": \n\n";
const CLOSE: &str = "event: close\n\n";

async fn boot() -> (App, std::net::SocketAddr) {
    let app = App::create(AppModule).await.unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = app.router();
    tokio::spawn(async move { axum::serve(listener, router).await });
    (app, addr)
}

/// One HTTP/1.0 request; returns `(head, body)` once the server closes the socket.
async fn request(addr: std::net::SocketAddr, method: &str, path: &str, body: Option<Value>) -> (String, String) {
    let mut socket = TcpStream::connect(addr).await.unwrap();
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let text = format!(
        "{method} {path} HTTP/1.0\r\nauthorization: Bearer dev-token\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{payload}",
        payload.len()
    );
    socket.write_all(text.as_bytes()).await.unwrap();
    let mut raw = String::new();
    timeout(Duration::from_secs(5), socket.read_to_string(&mut raw))
        .await
        .expect("the server should close the connection")
        .unwrap();
    let (head, body) = raw.split_once("\r\n\r\n").unwrap();
    (head.to_owned(), body.to_owned())
}

/// Opens the event stream and reads until `needle` shows up in what arrived.
async fn open_events(addr: std::net::SocketAddr) -> (TcpStream, String) {
    open_events_from(addr, None).await
}

/// Like [`open_events`], reconnecting with a `Last-Event-ID` header.
async fn open_events_from(addr: std::net::SocketAddr, last_event_id: Option<&str>) -> (TcpStream, String) {
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

/// Splits a message frame into its `id:` and its decoded `data:`.
fn message(frame: &str) -> (String, Value) {
    let (id, data) = frame.split_once('\n').expect("an id line and a data line");
    let id = id
        .strip_prefix("id: ")
        .unwrap_or_else(|| panic!("frame has no id: {frame:?}"));
    let data = data.strip_prefix("data: ").unwrap().trim_end();
    (id.to_owned(), serde_json::from_str(data).unwrap())
}

/// The sequence number of a cursor id (`<epoch>-<seq>`).
fn seq(id: &str) -> u64 {
    id.rsplit_once('-').unwrap().1.parse().unwrap()
}

/// The frames after the response head.
fn frames(seen: &str) -> Vec<&str> {
    seen.split_once("\r\n\r\n").unwrap().1.split_inclusive("\n\n").collect()
}

async fn user(addr: std::net::SocketAddr, name: &str) -> Value {
    let email = format!("{}@example.com", name.to_lowercase());
    let (_, body) = request(addr, "POST", "/users", Some(json!({ "name": name, "email": email }))).await;
    serde_json::from_str(&body).unwrap()
}

async fn publish(addr: std::net::SocketAddr, author: &Value, title: &str) -> Value {
    let body = json!({ "author_id": author["id"], "title": title, "body": "b" });
    let (_, post) = request(addr, "POST", "/posts", Some(body)).await;
    serde_json::from_str(&post).unwrap()
}

#[tokio::test]
async fn sends_flush_then_typed_messages_then_close() {
    let (app, addr) = boot().await;
    let (mut events, mut seen) = open_events(addr).await;

    // Headers arrive with the flush, before any event exists.
    let (head, first) = seen.split_once("\r\n\r\n").unwrap();
    assert!(head.starts_with("HTTP/1.0 200"), "{head}");
    assert!(
        head.to_lowercase().contains("content-type: text/event-stream"),
        "{head}"
    );
    assert_eq!(first, FLUSH);

    let (_, user) = request(
        addr,
        "POST",
        "/users",
        Some(json!({ "name": "Ada", "email": "ada@example.com" })),
    )
    .await;
    let user: Value = serde_json::from_str(&user).unwrap();
    let (_, post) = request(
        addr,
        "POST",
        "/posts",
        Some(json!({ "author_id": user["id"], "title": "Hello", "body": "World" })),
    )
    .await;
    let post: Value = serde_json::from_str(&post).unwrap();
    request(
        addr,
        "DELETE",
        &format!("/posts/{}", post["id"].as_str().unwrap()),
        None,
    )
    .await;

    read_until(&mut events, &mut seen, "\"type\":\"removed\"").await;

    // Stop the server: the open stream must finish with `close`, then EOF.
    assert!(app.shutdown().await.is_empty());
    let mut rest = String::new();
    timeout(Duration::from_secs(5), events.read_to_string(&mut rest))
        .await
        .expect("the stream should end on shutdown")
        .unwrap();
    seen.push_str(&rest);

    let body = seen.split_once("\r\n\r\n").unwrap().1;
    let frames: Vec<&str> = body.split_inclusive("\n\n").collect();
    assert_eq!(frames.len(), 4, "{frames:?}");
    assert_eq!(frames[0], FLUSH);
    let (first_id, published) = message(frames[1]);
    assert_eq!(
        published,
        json!({ "type": "published", "post": {
            "id": post["id"], "author_id": user["id"], "title": "Hello", "body": "World" } })
    );
    let (second_id, removed) = message(frames[2]);
    assert_eq!(removed, json!({ "type": "removed", "id": post["id"] }));
    assert_eq!(seq(&second_id), seq(&first_id) + 1, "ids count up by one");
    assert_eq!(frames[3], CLOSE);
}

#[tokio::test]
async fn reconnecting_with_last_event_id_replays_exactly_what_was_missed() {
    let (app, addr) = boot().await;
    let ada = user(addr, "Ada").await;

    // First connection sees p1 and p2, then drops.
    let (mut first, mut seen) = open_events(addr).await;
    publish(addr, &ada, "p1").await;
    publish(addr, &ada, "p2").await;
    read_until(&mut first, &mut seen, "\"title\":\"p2\"").await;
    let ids: Vec<String> = frames(&seen)[1..].iter().map(|frame| message(frame).0).collect();
    assert_eq!(ids.len(), 2);
    drop(first);

    // p3 and p4 happen while nobody is connected.
    publish(addr, &ada, "p3").await;
    publish(addr, &ada, "p4").await;

    // Reconnect after p2: p3 and p4 come first, then the stream is live.
    let (mut second, mut seen) = open_events_from(addr, Some(&ids[1])).await;
    read_until(&mut second, &mut seen, "\"title\":\"p4\"").await;
    publish(addr, &ada, "p5").await;
    read_until(&mut second, &mut seen, "\"title\":\"p5\"").await;

    let got: Vec<(String, Value)> = frames(&seen)[1..].iter().map(|frame| message(frame)).collect();
    let titles: Vec<&str> = got
        .iter()
        .map(|(_, data)| data["post"]["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, ["p3", "p4", "p5"], "no gap and no duplicate across the seam");
    let sequence: Vec<u64> = got.iter().map(|(id, _)| seq(id)).collect();
    assert_eq!(sequence, [seq(&ids[1]) + 1, seq(&ids[1]) + 2, seq(&ids[1]) + 3]);

    // The stream still ends properly.
    app.shutdown().await;
    let mut rest = String::new();
    timeout(Duration::from_secs(5), second.read_to_string(&mut rest))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rest, CLOSE);
}

#[tokio::test]
async fn unusable_last_event_ids_start_a_live_stream() {
    let (app, addr) = boot().await;
    let ada = user(addr, "Ada").await;
    publish(addr, &ada, "old").await;

    // Garbage, and a well-formed id from another server run: neither replays.
    for bogus in ["garbage", "1-1", "99999999999999999999-1"] {
        let (mut socket, mut seen) = open_events_from(addr, Some(bogus)).await;
        publish(addr, &ada, &format!("new-{bogus}")).await;
        read_until(&mut socket, &mut seen, &format!("new-{bogus}")).await;
        let titles: Vec<String> = frames(&seen)[1..]
            .iter()
            .map(|frame| message(frame).1["post"]["title"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(titles, [format!("new-{bogus}")], "{bogus:?} must not replay history");
    }
    app.shutdown().await;
}

#[tokio::test]
async fn every_subscriber_gets_every_event() {
    let (app, addr) = boot().await;
    let (mut one, mut one_seen) = open_events(addr).await;
    let (mut two, mut two_seen) = open_events(addr).await;
    let (_, user) = request(
        addr,
        "POST",
        "/users",
        Some(json!({ "name": "Bo", "email": "bo@example.com" })),
    )
    .await;
    let user: Value = serde_json::from_str(&user).unwrap();
    request(
        addr,
        "POST",
        "/posts",
        Some(json!({ "author_id": user["id"], "title": "T", "body": "B" })),
    )
    .await;
    read_until(&mut one, &mut one_seen, "\"type\":\"published\"").await;
    read_until(&mut two, &mut two_seen, "\"type\":\"published\"").await;
    app.shutdown().await;
}

#[tokio::test]
async fn the_openapi_document_describes_the_event_stream() {
    let app = App::create(AppModule).await.unwrap();
    let spec = serde_json::to_value(app.openapi(&ApiInfo::new("blog", "1"))).unwrap();
    let ok = &spec["paths"]["/posts/events"]["get"]["responses"]["200"]["content"];
    assert!(ok.get("application/json").is_none(), "{ok}");
    let variants = ok["text/event-stream"]["schema"]["oneOf"].as_array().expect("oneOf");
    let names: Vec<&Value> = variants.iter().map(|v| &v["properties"]["event"]["enum"][0]).collect();
    assert_eq!(names, ["message", "close", "error"]);
    assert_eq!(
        variants[0]["properties"]["data"]["$ref"],
        "#/components/schemas/PostEventDto"
    );
    assert!(spec["components"]["schemas"]["PostEventDto"].is_object());
}
