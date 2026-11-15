use nestrs_http::Interceptor;
use nestrs_http::axum::body::Body;
use nestrs_http::axum::extract::Request;
use nestrs_http::axum::http::{HeaderValue, Method, StatusCode, header};
use nestrs_http::axum::middleware::Next;
use nestrs_http::axum::response::Response;

use crate::support::{Snapshot, digest_hex};

/// Conditional `GET`: tags each `200` response with an `ETag` and answers a matching
/// `If-None-Match` with `304 Not Modified` and no body.
///
/// The tag is a weak validator (`W/"..."`) derived from a SHA-256 of the body, so it
/// changes exactly when the representation does. A handler that sets its own `ETag`
/// keeps it. Bodies that are streams, or larger than `max_body` (1 MiB by default),
/// are left alone and get no tag.
///
/// The handler still runs on every request; `ETag` saves bandwidth, not work. Pair it
/// with [`ResponseCache`](crate::ResponseCache) to save both.
#[derive(Debug, Clone, Copy)]
pub struct ETag {
    max_body: usize,
}

impl ETag {
    /// Tags bodies up to `bytes`.
    pub fn max_body(mut self, bytes: usize) -> Self {
        self.max_body = bytes;
        self
    }
}

impl Default for ETag {
    fn default() -> Self {
        Self { max_body: 1024 * 1024 }
    }
}

/// Whether an `If-None-Match` header value matches `tag` (weak comparison, `*` matches).
fn matches(if_none_match: &str, tag: &str) -> bool {
    let strip = |value: &str| value.trim().trim_start_matches("W/").to_owned();
    let tag = strip(tag);
    if_none_match.trim() == "*" || if_none_match.split(',').any(|candidate| strip(candidate) == tag)
}

impl Interceptor for ETag {
    async fn intercept(&self, request: Request, next: Next) -> Response {
        if !matches!(*request.method(), Method::GET | Method::HEAD) {
            return next.run(request).await;
        }
        let if_none_match = request
            .headers()
            .get(header::IF_NONE_MATCH)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let response = next.run(request).await;
        if response.status() != StatusCode::OK {
            return response;
        }

        let (mut response, tag) = match response.headers().get(header::ETAG).cloned() {
            Some(tag) => (response, tag),
            None => {
                let (response, snapshot) = Snapshot::capture(response, self.max_body).await;
                let Some(snapshot) = snapshot else {
                    return response;
                };
                let tag = format!("W/\"{}\"", digest_hex(&[&snapshot.body], 16));
                (response, HeaderValue::from_str(&tag).expect("hex is header-safe"))
            }
        };

        let hit = tag
            .to_str()
            .is_ok_and(|tag| if_none_match.as_deref().is_some_and(|inm| matches(inm, tag)));
        if hit {
            let mut not_modified = Response::new(Body::empty());
            *not_modified.status_mut() = StatusCode::NOT_MODIFIED;
            for name in [header::CACHE_CONTROL, header::VARY, header::EXPIRES] {
                if let Some(value) = response.headers().get(&name) {
                    not_modified.headers_mut().insert(name, value.clone());
                }
            }
            not_modified.headers_mut().insert(header::ETAG, tag);
            return not_modified;
        }
        response.headers_mut().insert(header::ETAG, tag);
        response
    }
}

#[cfg(test)]
mod tests {
    use crate::testing::{get, router_with, router_with_handler, text};

    use super::*;

    #[tokio::test]
    async fn tags_and_answers_conditional_requests() {
        let app = router_with(ETag::default());
        let first = get(&app, "/", &[]).await;
        let tag = first.headers()[header::ETAG].to_str().unwrap().to_owned();
        assert!(tag.starts_with("W/\""));
        assert_eq!(text(first).await, "ok");

        let again = get(&app, "/", &[("if-none-match", &tag)]).await;
        assert_eq!(again.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(again.headers()[header::ETAG].to_str().unwrap(), tag);
        assert_eq!(text(again).await, "");

        let stale = get(&app, "/", &[("if-none-match", "W/\"other\"")]).await;
        assert_eq!(stale.status(), StatusCode::OK);
        let any = get(&app, "/", &[("if-none-match", "*")]).await;
        assert_eq!(any.status(), StatusCode::NOT_MODIFIED);
    }

    #[tokio::test]
    async fn the_tag_follows_the_body_and_a_handler_tag_wins() {
        let a = get(&router_with_handler(ETag::default(), || async { "a" }), "/", &[]).await;
        let b = get(&router_with_handler(ETag::default(), || async { "b" }), "/", &[]).await;
        assert_ne!(a.headers()[header::ETAG], b.headers()[header::ETAG]);

        let own = router_with_handler(ETag::default(), || async { ([(header::ETAG, "\"mine\"")], "x") });
        assert_eq!(get(&own, "/", &[]).await.headers()[header::ETAG], "\"mine\"");
    }

    #[tokio::test]
    async fn oversized_bodies_are_left_untagged() {
        let app = router_with(ETag::default().max_body(1));
        let response = get(&app, "/", &[]).await;
        assert!(!response.headers().contains_key(header::ETAG));
        assert_eq!(text(response).await, "ok");
    }
}
