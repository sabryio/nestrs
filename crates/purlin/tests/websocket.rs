//! WebSocket routes: typed channel, route guards before the upgrade, graceful shutdown.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use purlin::http::axum::response::Response;
use purlin::http::{BearerAuth, Channel, StaticTokens, Ws};
use purlin::prelude::*;
use serde::{Deserialize, Serialize};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientMsg {
    Say { text: String },
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ServerMsg {
    Said { by: String, text: String },
    Error { message: String },
}

#[injectable]
struct Chat;

#[controller("/chat")]
#[guard(BearerAuth)]
impl Chat {
    #[get("/ws")]
    async fn connect(&self, ws: Ws, principal: Principal) -> Response {
        let by = principal.subject().to_owned();
        ws.on_upgrade(move |mut channel: Channel<ClientMsg, ServerMsg>| async move {
            while let Some(message) = channel.recv().await {
                let reply = match message {
                    Ok(ClientMsg::Say { text }) => ServerMsg::Said { by: by.clone(), text },
                    Err(error) => ServerMsg::Error {
                        message: error.to_string(),
                    },
                };
                if channel.send(&reply).await.is_err() {
                    break;
                }
            }
        })
    }
}

#[module(
    providers = [StaticTokens::new().token("t-ann", Principal::new("ann")).provider(), BearerAuth],
    controllers = [Chat],
)]
struct AppModule;

async fn serve() -> (App, std::net::SocketAddr) {
    let app = App::create(AppModule).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = app.router();
    tokio::spawn(async move { purlin::http::axum::serve(listener, router).await });
    (app, addr)
}

fn request(addr: std::net::SocketAddr, token: Option<&str>) -> impl IntoClientRequest {
    let mut request = format!("ws://{addr}/chat/ws").into_client_request().unwrap();
    if let Some(token) = token {
        request
            .headers_mut()
            .insert("authorization", format!("Bearer {token}").parse().unwrap());
    }
    request
}

async fn next_text<S>(socket: &mut S) -> String
where
    S: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        match tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await
            .unwrap()
        {
            Some(Ok(Message::Text(text))) => return text.as_str().to_owned(),
            Some(Ok(_)) => continue,
            other => panic!("expected a text frame, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn typed_messages_round_trip_and_bad_input_keeps_the_connection() {
    let (_app, addr) = serve().await;
    let (mut socket, _) = tokio_tungstenite::connect_async(request(addr, Some("t-ann")))
        .await
        .unwrap();

    socket
        .send(Message::text(r#"{"type":"say","text":"hi"}"#))
        .await
        .unwrap();
    assert_eq!(
        next_text(&mut socket).await,
        r#"{"type":"said","by":"ann","text":"hi"}"#
    );

    socket.send(Message::text("not json")).await.unwrap();
    assert!(next_text(&mut socket).await.contains(r#""type":"error""#));

    // Still open after the bad frame.
    socket
        .send(Message::text(r#"{"type":"say","text":"again"}"#))
        .await
        .unwrap();
    assert!(next_text(&mut socket).await.contains("again"));
}

#[tokio::test]
async fn the_route_guard_runs_before_the_upgrade() {
    let (_app, addr) = serve().await;
    let error = tokio_tungstenite::connect_async(request(addr, None)).await.unwrap_err();
    match error {
        tokio_tungstenite::tungstenite::Error::Http(response) => assert_eq!(response.status(), 401),
        other => panic!("expected an HTTP 401, got {other:?}"),
    }
}

#[tokio::test]
async fn shutdown_closes_open_sockets_with_going_away() {
    let (app, addr) = serve().await;
    // Starting shutdown is what flips the signal; see App::shutdown.
    let (mut socket, _) = tokio_tungstenite::connect_async(request(addr, Some("t-ann")))
        .await
        .unwrap();
    socket
        .send(Message::text(r#"{"type":"say","text":"x"}"#))
        .await
        .unwrap();
    next_text(&mut socket).await;

    app.shutdown().await;

    let frame = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .unwrap();
    match frame {
        Some(Ok(Message::Close(Some(close)))) => {
            assert_eq!(u16::from(close.code), 1001);
        }
        other => panic!("expected a going-away close, got {other:?}"),
    }
}
