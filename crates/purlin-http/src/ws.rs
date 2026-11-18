//! Typed WebSockets.
//!
//! A WebSocket is an ordinary `GET` route that upgrades, so everything a route has
//! already applies: `#[guard]`, `#[roles]`, `#[interceptor]` and the scopes all run
//! *before* the upgrade, and a rejected request never becomes a socket. This module
//! only adds the part that is different, the conversation after the upgrade:
//!
//! ```ignore
//! #[get("/chat")]
//! #[guard(BearerAuth)]
//! async fn chat(&self, ws: Ws, Principal(user): Principal) -> Response {
//!     let rooms = self.rooms.clone();
//!     ws.on_upgrade(move |mut channel: Channel<ClientMsg, ServerMsg>| async move {
//!         while let Some(message) = channel.recv().await {
//!             match message {
//!                 Ok(ClientMsg::Say { text }) => { let _ = channel.send(&ServerMsg::Said { text }).await; }
//!                 Err(error) => { let _ = channel.send(&ServerMsg::Error(error.to_string())).await; }
//!             }
//!         }
//!     })
//! }
//! ```
//!
//! ## Contract of [`Channel`]
//!
//! * Messages are JSON text frames, decoded to `In` and encoded from `Out`; the
//!   handler never sees a frame type.
//! * Ping and pong are answered by the transport; they never reach the handler.
//! * A frame that is not valid for `In` yields `Some(Err(..))` and the connection
//!   stays open: whether bad input is fatal is the handler's decision.
//! * [`recv`](Channel::recv) returns `None` when the peer closes, the transport
//!   fails, or the app starts shutting down (the peer is sent a `1001 Going Away`
//!   close frame first), so a `while let` loop is all a handler needs to drain.
//!
//! ## Design
//!
//! * [`Ws`] only upgrades; [`Channel`] only converses (SRP).
//! * Nothing here knows about guards or interceptors: they are the route's, which
//!   keeps the cross-cutting pipeline in one place (OCP, DRY).

use std::fmt;
use std::future::Future;
use std::marker::PhantomData;

use axum::extract::FromRequestParts;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade, close_code};
use axum::http::request::Parts;
use axum::response::Response;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::{HttpError, ShuttingDown};

/// Why a message could not be received or sent.
#[derive(Debug)]
#[non_exhaustive]
pub enum WsError {
    /// The frame was not valid JSON for the expected type.
    Decode(serde_json::Error),
    /// A binary frame arrived; this channel speaks JSON text only.
    Binary,
    /// The message could not be encoded.
    Encode(serde_json::Error),
    /// The connection is gone.
    Closed,
}

impl fmt::Display for WsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Decode(error) => write!(f, "invalid message: {error}"),
            Self::Binary => f.write_str("binary frames are not supported"),
            Self::Encode(error) => write!(f, "cannot encode message: {error}"),
            Self::Closed => f.write_str("connection closed"),
        }
    }
}

impl std::error::Error for WsError {}

/// The WebSocket upgrade, as a route argument.
pub struct Ws {
    upgrade: WebSocketUpgrade,
    shutdown: ShuttingDown,
}

impl Ws {
    /// Upgrades the connection and runs `handler` with a typed [`Channel`].
    pub fn on_upgrade<In, Out, F, Fut>(self, handler: F) -> Response
    where
        In: DeserializeOwned + Send + 'static,
        Out: Serialize + Send + 'static,
        F: FnOnce(Channel<In, Out>) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let shutdown = self.shutdown;
        self.upgrade.on_upgrade(move |socket| {
            handler(Channel {
                socket,
                shutdown,
                closed: false,
                _types: PhantomData,
            })
        })
    }
}

impl<S: Send + Sync> FromRequestParts<S> for Ws {
    type Rejection = HttpError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let shutdown = ShuttingDown::from_request_parts(parts, state).await?;
        if shutdown.is_set() {
            return Err(HttpError::unavailable("the server is shutting down", None));
        }
        let upgrade = WebSocketUpgrade::from_request_parts(parts, state)
            .await
            .map_err(|rejection| HttpError::bad_request(rejection.body_text()))?;
        Ok(Self { upgrade, shutdown })
    }
}

/// A typed, shutdown-aware conversation over an upgraded socket.
pub struct Channel<In, Out> {
    socket: WebSocket,
    shutdown: ShuttingDown,
    closed: bool,
    _types: PhantomData<fn() -> (In, Out)>,
}

impl<In: DeserializeOwned, Out: Serialize> Channel<In, Out> {
    /// The next message from the peer, or `None` once the conversation is over.
    pub async fn recv(&mut self) -> Option<Result<In, WsError>> {
        loop {
            if self.closed {
                return None;
            }
            let frame = tokio::select! {
                biased;
                () = self.shutdown.wait() => {
                    self.close(close_code::AWAY, "server shutting down").await;
                    return None;
                }
                frame = self.socket.recv() => frame,
            };
            match frame {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => {
                    self.closed = true;
                    return None;
                }
                Some(Ok(Message::Text(text))) => {
                    return Some(serde_json::from_str(text.as_str()).map_err(WsError::Decode));
                }
                Some(Ok(Message::Binary(_))) => return Some(Err(WsError::Binary)),
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
            }
        }
    }

    /// Sends one message.
    pub async fn send(&mut self, message: &Out) -> Result<(), WsError> {
        if self.closed {
            return Err(WsError::Closed);
        }
        let text = serde_json::to_string(message).map_err(WsError::Encode)?;
        self.socket.send(Message::Text(text.into())).await.map_err(|_| {
            self.closed = true;
            WsError::Closed
        })
    }

    /// Closes the conversation with a close code and reason. Idempotent.
    pub async fn close(&mut self, code: u16, reason: &str) {
        if !self.closed {
            self.closed = true;
            let frame = CloseFrame {
                code,
                reason: reason.to_owned().into(),
            };
            let _ = self.socket.send(Message::Close(Some(frame))).await;
        }
    }
}
