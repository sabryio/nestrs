use http_body_util::Limited;
use purlin_http::Interceptor;
use purlin_http::axum::body::Body;
use purlin_http::axum::extract::Request;
use purlin_http::axum::http::{StatusCode, header};
use purlin_http::axum::middleware::Next;
use purlin_http::axum::response::Response;

use crate::support::error_response;

/// Rejects request bodies larger than a limit, per route.
///
/// A declared `content-length` over the limit is refused immediately with `413` and
/// code `payload_too_large`, before any of the body is read. A body that lies about
/// its length, or has none (chunked), is cut off while it is read, so extractors fail
/// with `413` too.
///
/// ```ignore
/// #[post] #[interceptor(BodyLimit::kib(64))]
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BodyLimit {
    max: usize,
}

impl BodyLimit {
    /// A limit of `bytes`.
    ///
    /// # Panics
    /// If `bytes` is zero.
    pub fn bytes(bytes: usize) -> Self {
        assert!(bytes > 0, "BodyLimit needs a limit greater than zero");
        Self { max: bytes }
    }

    /// A limit in kibibytes.
    pub fn kib(kib: usize) -> Self {
        Self::bytes(kib * 1024)
    }

    /// A limit in mebibytes.
    pub fn mib(mib: usize) -> Self {
        Self::bytes(mib * 1024 * 1024)
    }
}

impl Interceptor for BodyLimit {
    async fn intercept(&self, request: Request, next: Next) -> Response {
        let declared = request
            .headers()
            .get(header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<usize>().ok());
        if declared.is_some_and(|length| length > self.max) {
            return too_large(self.max);
        }
        let (parts, body) = request.into_parts();
        let request = Request::from_parts(parts, Body::new(Limited::new(body, self.max)));
        next.run(request).await
    }
}

fn too_large(max: usize) -> Response {
    error_response(
        StatusCode::PAYLOAD_TOO_LARGE,
        "payload_too_large",
        "the request body is too large",
        Some(serde_json::json!({ "max_bytes": max })),
    )
}

#[cfg(test)]
mod tests {
    use purlin_http::axum::http::StatusCode;

    use crate::testing::{post, router_with_echo};

    use super::*;

    #[tokio::test]
    async fn rejects_by_declared_length_and_by_streamed_length() {
        let app = router_with_echo(BodyLimit::bytes(8));
        assert_eq!(post(&app, "/", "12345678").await.status(), StatusCode::OK);
        let rejected = post(&app, "/", "123456789").await;
        assert_eq!(rejected.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn a_body_without_a_declared_length_is_cut_off_while_read() {
        use purlin_http::axum::http::Request as HttpRequest;
        use tower::ServiceExt;

        let app = router_with_echo(BodyLimit::bytes(4));
        let chunks = purlin_http::stream::iter(vec![Ok::<_, std::io::Error>(b"abc".to_vec()), Ok(b"def".to_vec())]);
        let request = HttpRequest::post("/").body(Body::from_stream(chunks)).unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }
}
