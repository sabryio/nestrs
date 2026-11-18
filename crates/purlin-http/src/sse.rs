//! Typed Server-Sent Events.
//!
//! [`EventStream<T>`] turns a [`Stream`] of `T` into a `text/event-stream` response
//! and documents `T` in the OpenAPI spec, so a handler's return type *is* the event
//! contract:
//!
//! ```ignore
//! #[get("/events")]
//! async fn events(&self) -> EventStream<PostEventDto> {
//!     EventStream::new(self.watch.stream())
//! }
//! ```
//!
//! ## Wire format
//!
//! The format is the one oRPC's event iterators speak, so an `OpenAPILink` client
//! consumes it as a typed `AsyncIterator`:
//!
//! | frame                         | meaning                                   |
//! |-------------------------------|-------------------------------------------|
//! | `: \n\n`                      | flush: first bytes, and the keep-alive    |
//! | `data: <json>\n\n`            | one message                               |
//! | `id: <id>\ndata: <json>\n\n` | one message the client can resume after   |
//! | `event: close\n\n`            | the stream completed (always last)        |
//! | `event: error\ndata: <json>\n\n` | the stream failed (always last)        |
//!
//! Every stream sends the flush first (so proxies and browsers see headers and a
//! first chunk immediately), then its messages, then exactly one terminal frame.
//! The terminal frame is also sent when the server shuts down, which is what lets
//! graceful shutdown finish instead of waiting on open connections forever.
//!
//! ## Resuming
//!
//! Give a message an id with [`Event::with_id`]. A client that loses the connection
//! reconnects with a `Last-Event-ID` header carrying the last id it saw (the browser
//! `EventSource` and oRPC's `RetryLinkPlugin` both do this). Read it with the
//! [`LastEventId`] extractor and start the new stream *after* that id, so nothing is
//! missed and nothing repeats. Ids are opaque to this module.
//!
//! ## Design
//!
//! * [`EventStream`] only encodes; it knows nothing about shutdown (SRP).
//! * Shutdown is applied by a middleware installed by `App`, which wraps any
//!   `text/event-stream` body in [`Terminating`] (OCP: handlers don't change).

use std::convert::Infallible;
use std::fmt;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE, HeaderValue};
use axum::response::{IntoResponse, Response};
use futures_util::stream::{BoxStream, Stream, StreamExt};
use serde::Serialize;
use tokio::sync::watch;
use tokio::time::{Instant, Sleep};

use crate::HttpError;

/// First bytes of every stream, and the keep-alive frame.
pub const FLUSH: &str = ": \n\n";
/// The terminal frame of a stream that completed.
pub const CLOSE: &str = "event: close\n\n";
/// How long a stream may stay silent before a keep-alive is sent.
pub const KEEP_ALIVE: Duration = Duration::from_secs(15);

/// One message of a stream: the payload plus an optional resume id.
#[derive(Debug, Clone)]
pub struct Event<T> {
    id: Option<String>,
    data: T,
}

impl<T> Event<T> {
    /// A message without an id (not resumable).
    pub fn new(data: T) -> Self {
        Self { id: None, data }
    }

    /// A message a client can resume after. Line breaks are not allowed in SSE
    /// fields, so they are dropped from the id.
    pub fn with_id(id: impl fmt::Display, data: T) -> Self {
        let id: String = id.to_string().chars().filter(|c| !matches!(c, '\n' | '\r')).collect();
        Self { id: Some(id), data }
    }
}

impl<T> From<T> for Event<T> {
    fn from(data: T) -> Self {
        Self::new(data)
    }
}

/// The `Last-Event-ID` request header: the id of the last message the client saw
/// before it reconnected, or `None` on a first connection.
///
/// ```ignore
/// #[get("/events")]
/// async fn events(&self, last: LastEventId) -> EventStream<MyEvent> {
///     let after = last.parse::<MyCursor>();   // None if absent or malformed
///     ..
/// }
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LastEventId(pub Option<String>);

impl LastEventId {
    /// The id parsed as `C`. A missing or malformed header is `None`: the client
    /// simply gets a fresh stream instead of an error.
    pub fn parse<C: std::str::FromStr>(&self) -> Option<C> {
        self.0.as_deref()?.parse().ok()
    }
}

impl<S: Send + Sync> axum::extract::FromRequestParts<S> for LastEventId {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut axum::http::request::Parts, _: &S) -> Result<Self, Infallible> {
        let id = parts
            .headers
            .get("last-event-id")
            .and_then(|value| value.to_str().ok())
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        Ok(Self(id))
    }
}

/// A typed event stream response. `T` is the message type and is what the OpenAPI
/// document advertises for the route.
pub struct EventStream<T> {
    source: BoxStream<'static, Result<Event<T>, HttpError>>,
    keep_alive: Duration,
}

impl<T: Serialize + Send + 'static> EventStream<T> {
    /// A stream that cannot fail.
    pub fn new<S>(stream: S) -> Self
    where
        S: Stream<Item = T> + Send + 'static,
    {
        Self::from_events(stream.map(|data| Ok(Event::new(data))))
    }

    /// A stream whose items may fail. The first `Err` is sent as an `error` frame
    /// and ends the stream.
    pub fn try_new<S>(stream: S) -> Self
    where
        S: Stream<Item = Result<T, HttpError>> + Send + 'static,
    {
        Self::from_events(stream.map(|item| item.map(Event::new)))
    }

    /// A resumable stream: every [`Event`] may carry an id, and items may fail.
    pub fn from_events<S>(stream: S) -> Self
    where
        S: Stream<Item = Result<Event<T>, HttpError>> + Send + 'static,
    {
        Self {
            source: stream.boxed(),
            keep_alive: KEEP_ALIVE,
        }
    }

    /// Overrides the keep-alive interval (default 15 s).
    pub fn keep_alive(mut self, interval: Duration) -> Self {
        self.keep_alive = interval;
        self
    }

    /// The encoded frames, one `Bytes` per frame.
    pub fn into_frames(self) -> impl Stream<Item = Bytes> + Send + 'static {
        Encoder::new(self.source, self.keep_alive)
    }
}

impl<T> fmt::Debug for EventStream<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EventStream").finish_non_exhaustive()
    }
}

impl<T: Serialize + Send + 'static> IntoResponse for EventStream<T> {
    fn into_response(self) -> Response {
        let body = Body::from_stream(self.into_frames().map(Ok::<_, Infallible>));
        let mut response = Response::new(body);
        let headers = response.headers_mut();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
        headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
        // Tells reverse proxies (nginx) not to buffer the stream.
        headers.insert("x-accel-buffering", HeaderValue::from_static("no"));
        response
    }
}

/// Frames a message: `[id: <id>\n]data: <json>\n\n`.
fn message_frame<T: Serialize>(event: &Event<T>) -> Result<Bytes, serde_json::Error> {
    let json = serde_json::to_string(&event.data)?;
    Ok(Bytes::from(match &event.id {
        Some(id) => format!("id: {id}\ndata: {json}\n\n"),
        None => format!("data: {json}\n\n"),
    }))
}

/// Frames a failure: `event: error` with `{"code","message"}` data.
fn error_frame(error: &HttpError) -> Bytes {
    let data = serde_json::json!({ "code": error.code(), "message": error.to_string() });
    Bytes::from(format!("event: error\ndata: {data}\n\n"))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Start,
    Running,
    Done,
}

/// `flush, message*, (close | error)` as a stream of frames.
struct Encoder<T> {
    source: BoxStream<'static, Result<Event<T>, HttpError>>,
    phase: Phase,
    interval: Duration,
    /// Created on first poll, so building a response needs no runtime.
    idle: Option<Pin<Box<Sleep>>>,
}

impl<T> Encoder<T> {
    fn new(source: BoxStream<'static, Result<Event<T>, HttpError>>, interval: Duration) -> Self {
        Self {
            source,
            phase: Phase::Start,
            interval,
            idle: None,
        }
    }

    fn rearm(&mut self) {
        let deadline = Instant::now() + self.interval;
        match self.idle.as_mut() {
            Some(idle) => idle.as_mut().reset(deadline),
            None => self.idle = Some(Box::pin(tokio::time::sleep_until(deadline))),
        }
    }
}

impl<T: Serialize> Stream for Encoder<T> {
    type Item = Bytes;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Bytes>> {
        let this = &mut *self;
        match this.phase {
            Phase::Done => return Poll::Ready(None),
            Phase::Start => {
                this.phase = Phase::Running;
                this.rearm();
                return Poll::Ready(Some(Bytes::from_static(FLUSH.as_bytes())));
            }
            Phase::Running => {}
        }
        match this.source.poll_next_unpin(cx) {
            Poll::Ready(Some(Ok(value))) => {
                this.rearm();
                match message_frame(&value) {
                    Ok(frame) => Poll::Ready(Some(frame)),
                    Err(error) => {
                        tracing::error!(%error, "could not serialise an event");
                        this.phase = Phase::Done;
                        Poll::Ready(Some(error_frame(&HttpError::Internal(
                            "event serialisation failed".into(),
                        ))))
                    }
                }
            }
            Poll::Ready(Some(Err(error))) => {
                this.phase = Phase::Done;
                Poll::Ready(Some(error_frame(&error)))
            }
            Poll::Ready(None) => {
                this.phase = Phase::Done;
                Poll::Ready(Some(Bytes::from_static(CLOSE.as_bytes())))
            }
            Poll::Pending => {
                let expired = this.idle.as_mut().is_some_and(|idle| idle.as_mut().poll(cx).is_ready());
                if expired {
                    this.rearm();
                    // Register the new deadline's waker.
                    if let Some(idle) = this.idle.as_mut() {
                        let _ = idle.as_mut().poll(cx);
                    }
                    return Poll::Ready(Some(Bytes::from_static(FLUSH.as_bytes())));
                }
                Poll::Pending
            }
        }
    }
}

use std::future::Future;

/// Wraps an SSE body so that, once the server shuts down, it ends with a `close`
/// frame instead of keeping the connection open.
pub(crate) struct Terminating {
    inner: BoxStream<'static, Result<Bytes, axum::Error>>,
    shutdown: watch::Receiver<bool>,
    notified: Option<Pin<Box<dyn Future<Output = ()> + Send>>>,
    done: bool,
}

impl Terminating {
    fn new(body: Body, shutdown: watch::Receiver<bool>) -> Self {
        let mut watcher = shutdown.clone();
        let notified: Pin<Box<dyn Future<Output = ()> + Send>> = Box::pin(async move {
            // Resolves when the flag turns true (or immediately if it already is).
            let _ = watcher.wait_for(|stopped| *stopped).await;
        });
        Self {
            inner: body.into_data_stream().boxed(),
            shutdown,
            notified: Some(notified),
            done: false,
        }
    }
}

impl Stream for Terminating {
    type Item = Result<Bytes, axum::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = &mut *self;
        if this.done {
            return Poll::Ready(None);
        }
        if let Some(notified) = this.notified.as_mut()
            && notified.as_mut().poll(cx).is_ready()
        {
            this.done = true;
            this.notified = None;
            let _ = &this.shutdown;
            return Poll::Ready(Some(Ok(Bytes::from_static(CLOSE.as_bytes()))));
        }
        match this.inner.poll_next_unpin(cx) {
            Poll::Ready(None) => {
                this.done = true;
                Poll::Ready(None)
            }
            other => other,
        }
    }
}

/// Whether the app has begun shutting down. Extract it in a handler to stop taking
/// new work early (readiness probes answer 503 once it is true, so load balancers
/// drain traffic before the server closes).
#[derive(Debug, Clone)]
pub struct ShuttingDown(watch::Receiver<bool>);

impl ShuttingDown {
    /// `true` once shutdown has started.
    pub fn is_set(&self) -> bool {
        *self.0.borrow()
    }

    /// Resolves once shutdown has started (immediately if it already has).
    pub async fn wait(&mut self) {
        let _ = self.0.wait_for(|stopped| *stopped).await;
    }
}

impl<S: Send + Sync> axum::extract::FromRequestParts<S> for ShuttingDown {
    type Rejection = crate::HttpError;

    async fn from_request_parts(parts: &mut axum::http::request::Parts, _: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<Self>()
            .cloned()
            .ok_or_else(|| crate::HttpError::internal("ShuttingDown requested outside an App"))
    }
}

/// Middleware: makes every `text/event-stream` response end on shutdown, and tells
/// handlers whether shutdown has started.
pub(crate) async fn end_on_shutdown(
    axum::extract::State(shutdown): axum::extract::State<watch::Receiver<bool>>,
    mut request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    request.extensions_mut().insert(ShuttingDown(shutdown.clone()));
    let response = next.run(request).await;
    let is_sse = response
        .headers()
        .get(CONTENT_TYPE)
        .is_some_and(|value| value.as_bytes().starts_with(b"text/event-stream"));
    if !is_sse {
        return response;
    }
    let (parts, body) = response.into_parts();
    Response::from_parts(parts, Body::from_stream(Terminating::new(body, shutdown)))
}

#[cfg(test)]
mod tests {
    use futures_util::stream;

    use super::*;

    #[derive(Serialize)]
    struct Tick {
        n: u32,
    }

    async fn collect<T: Serialize + Send + 'static>(stream: EventStream<T>) -> Vec<String> {
        stream
            .into_frames()
            .map(|bytes| String::from_utf8(bytes.to_vec()).unwrap())
            .collect()
            .await
    }

    #[tokio::test]
    async fn sends_flush_then_messages_then_close() {
        let frames = collect(EventStream::new(stream::iter([Tick { n: 1 }, Tick { n: 2 }]))).await;
        assert_eq!(
            frames,
            [
                ": \n\n",
                "data: {\"n\":1}\n\n",
                "data: {\"n\":2}\n\n",
                "event: close\n\n"
            ]
        );
    }

    #[tokio::test]
    async fn ids_are_framed_before_the_data() {
        let events = stream::iter([
            Ok(Event::with_id("7-1", Tick { n: 1 })),
            Ok(Event::with_id("a\nb", Tick { n: 2 })),
        ]);
        let frames = collect(EventStream::from_events(events)).await;
        assert_eq!(frames[1], "id: 7-1\ndata: {\"n\":1}\n\n");
        assert_eq!(
            frames[2], "id: ab\ndata: {\"n\":2}\n\n",
            "line breaks never reach the wire"
        );
    }

    #[tokio::test]
    async fn last_event_id_reads_the_header() {
        use axum::extract::FromRequestParts;
        let extract = |header: Option<&str>| {
            let mut request = axum::http::Request::builder();
            if let Some(value) = header {
                request = request.header("last-event-id", value);
            }
            let (mut parts, ()) = request.body(()).unwrap().into_parts();
            async move { LastEventId::from_request_parts(&mut parts, &()).await.unwrap() }
        };
        assert_eq!(extract(None).await, LastEventId(None));
        assert_eq!(extract(Some("")).await, LastEventId(None));
        let seen = extract(Some("12")).await;
        assert_eq!(seen.parse::<u64>(), Some(12));
        assert_eq!(seen.parse::<bool>(), None, "malformed ids are ignored, not errors");
    }

    #[tokio::test]
    async fn an_empty_stream_is_flush_then_close() {
        let frames = collect(EventStream::<Tick>::new(stream::empty())).await;
        assert_eq!(frames, [": \n\n", "event: close\n\n"]);
    }

    #[tokio::test]
    async fn an_error_ends_the_stream_without_close() {
        let items = vec![Ok(Tick { n: 1 }), Err(HttpError::not_found("gone")), Ok(Tick { n: 2 })];
        let frames = collect(EventStream::try_new(stream::iter(items))).await;
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[1], "data: {\"n\":1}\n\n");
        assert!(frames[2].starts_with("event: error\ndata: "), "{}", frames[2]);
        assert!(frames[2].contains("\"code\":\"not_found\""));
    }

    #[tokio::test(start_paused = true)]
    async fn idle_streams_send_keep_alives() {
        let (tx, rx) = tokio::sync::mpsc::channel::<Tick>(1);
        let source = stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|tick| (tick, rx)) });
        let mut frames = Box::pin(
            EventStream::new(source)
                .keep_alive(Duration::from_secs(5))
                .into_frames(),
        );
        assert_eq!(frames.next().await.unwrap(), FLUSH.as_bytes());
        // Nothing to say for 5 s: the next frame is a keep-alive comment.
        assert_eq!(frames.next().await.unwrap(), FLUSH.as_bytes());
        tx.send(Tick { n: 7 }).await.unwrap();
        assert_eq!(frames.next().await.unwrap(), "data: {\"n\":7}\n\n".as_bytes());
        drop(tx);
        assert_eq!(frames.next().await.unwrap(), CLOSE.as_bytes());
        assert!(frames.next().await.is_none());
    }

    #[tokio::test]
    async fn shutdown_closes_an_open_stream() {
        let (stop, signal) = watch::channel(false);
        let open = EventStream::new(stream::pending::<Tick>()).into_response();
        let (_, body) = open.into_parts();
        let mut wrapped = Box::pin(Terminating::new(body, signal));
        assert_eq!(wrapped.next().await.unwrap().unwrap(), FLUSH.as_bytes());
        stop.send(true).unwrap();
        assert_eq!(wrapped.next().await.unwrap().unwrap(), CLOSE.as_bytes());
        assert!(wrapped.next().await.is_none());
    }

    #[test]
    fn responses_carry_the_sse_headers() {
        let response = EventStream::new(stream::empty::<Tick>()).into_response();
        assert_eq!(response.headers()[CONTENT_TYPE], "text/event-stream");
        assert_eq!(response.headers()[CACHE_CONTROL], "no-cache");
    }
}
