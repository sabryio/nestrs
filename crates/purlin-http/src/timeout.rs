use std::time::Duration;

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::{HttpError, Interceptor};

/// Gives a route a deadline: if the rest of the pipeline has not produced a response
/// within the limit, the caller gets `504` with code `timeout` instead of waiting.
///
/// ```ignore
/// #[interceptor(Timeout::secs(5))]          // on a controller or a route
/// // or for the whole app, outside guards and filters of every route:
/// app.interceptor(Timeout::secs(30))
/// ```
///
/// What the deadline covers and what it does not:
///
/// - It covers everything up to the response *head*: reading and parsing the body,
///   extractors, the handler. A slow upload counts.
/// - It does **not** cover streaming the body afterwards. A Server-Sent Events route
///   returns its head immediately, so `Timeout` never cuts a live stream.
/// - When it fires, the handler future is dropped at its next `.await`. Handlers
///   must therefore be cancel-safe: a write that already reached the database is
///   not undone. Blocking CPU work between awaits cannot be interrupted at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timeout {
    limit: Duration,
}

impl Timeout {
    /// A deadline of `limit`.
    ///
    /// # Panics
    /// If `limit` is zero: every request would time out, which is a typo, not a policy.
    pub fn new(limit: Duration) -> Self {
        assert!(!limit.is_zero(), "Timeout needs a limit greater than zero");
        Self { limit }
    }

    /// A deadline in whole seconds.
    pub fn secs(seconds: u64) -> Self {
        Self::new(Duration::from_secs(seconds))
    }

    /// A deadline in milliseconds.
    pub fn millis(milliseconds: u64) -> Self {
        Self::new(Duration::from_millis(milliseconds))
    }

    /// The configured deadline.
    pub fn limit(&self) -> Duration {
        self.limit
    }
}

impl Interceptor for Timeout {
    async fn intercept(&self, request: Request, next: Next) -> Response {
        match tokio::time::timeout(self.limit, next.run(request)).await {
            Ok(response) => response,
            Err(_) => {
                tracing::warn!(limit = ?self.limit, "request timed out");
                HttpError::Timeout.into_response()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use futures_util::stream;
    use tower::ServiceExt;

    use super::*;
    use crate::EventStream;

    fn app(limit: Timeout) -> Router {
        let limit = std::sync::Arc::new(limit);
        let wrap = |route| crate::intercepted(route, limit.clone());
        Router::new()
            .route("/fast", wrap(get(|| async { "ok" })))
            .route(
                "/slow",
                wrap(get(|| async {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    "late"
                })),
            )
            .route(
                "/events",
                wrap(get(|| async { EventStream::new(stream::pending::<u32>()) })),
            )
    }

    async fn get_status(router: &Router, path: &str) -> (StatusCode, String) {
        let response = router
            .clone()
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        (
            status,
            String::from_utf8(to_bytes(response.into_body(), usize::MAX).await.unwrap().to_vec()).unwrap(),
        )
    }

    #[tokio::test(start_paused = true)]
    async fn fast_handlers_pass_and_slow_ones_get_a_504_envelope() {
        let router = app(Timeout::secs(5));
        assert_eq!(get_status(&router, "/fast").await, (StatusCode::OK, "ok".into()));

        let (status, body) = get_status(&router, "/slow").await;
        assert_eq!(status, StatusCode::GATEWAY_TIMEOUT);
        let body: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(body["error"]["code"], "timeout");
    }

    #[tokio::test(start_paused = true)]
    async fn the_deadline_applies_to_the_response_head_not_to_a_live_stream() {
        let router = app(Timeout::millis(100));
        let response = router
            .clone()
            .oneshot(Request::get("/events").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Long after the deadline, the stream is still open and still delivers.
        tokio::time::sleep(Duration::from_secs(30)).await;
        let mut body = response.into_body().into_data_stream();
        let first = futures_util::StreamExt::next(&mut body).await.unwrap().unwrap();
        assert_eq!(&first[..], b": \n\n");
    }

    #[test]
    #[should_panic(expected = "greater than zero")]
    fn a_zero_deadline_is_rejected_at_construction() {
        let _ = Timeout::new(Duration::ZERO);
    }
}
