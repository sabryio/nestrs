use nestrs_http::Interceptor;
use nestrs_http::axum::extract::Request;
use nestrs_http::axum::http::{HeaderName, HeaderValue};
use nestrs_http::axum::middleware::Next;
use nestrs_http::axum::response::Response;
use tracing::Instrument;
use uuid::Uuid;

/// The request id of the current request. Take it as a handler parameter once
/// [`RequestId`] is installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestIdValue(pub String);

extension_extractor!(RequestIdValue, "RequestId");

/// Gives every request an id, echoes it on the response and puts it on the tracing span.
///
/// The caller's `x-request-id` is kept when it is 1 to 128 visible ASCII characters;
/// otherwise a UUID is generated. The id is also written back into the *request*
/// headers, so anything downstream that reads `x-request-id` (logging, the kit's
/// `RequestContext`) agrees with what the client sees.
///
/// Install it first (outermost): `app.interceptor(RequestId::default())`.
#[derive(Debug, Clone)]
pub struct RequestId {
    header: HeaderName,
}

impl RequestId {
    /// Uses a different header name than `x-request-id`.
    pub fn header(name: &'static str) -> Self {
        Self {
            header: HeaderName::from_static(name),
        }
    }
}

impl Default for RequestId {
    fn default() -> Self {
        Self::header("x-request-id")
    }
}

fn acceptable(value: &str) -> bool {
    (1..=128).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_graphic())
}

impl Interceptor for RequestId {
    async fn intercept(&self, mut request: Request, next: Next) -> Response {
        let id = request
            .headers()
            .get(&self.header)
            .and_then(|value| value.to_str().ok())
            .filter(|value| acceptable(value))
            .map_or_else(|| Uuid::new_v4().to_string(), str::to_owned);
        let value = HeaderValue::from_str(&id).expect("validated or generated ids are header-safe");
        request.headers_mut().insert(self.header.clone(), value.clone());
        request.extensions_mut().insert(RequestIdValue(id.clone()));

        let span = tracing::info_span!("request", request_id = %id);
        let mut response = next.run(request).instrument(span).await;
        response.headers_mut().insert(self.header.clone(), value);
        response
    }
}

#[cfg(test)]
mod tests {
    use crate::testing::{get, router_with};

    use super::*;

    #[tokio::test]
    async fn keeps_a_good_id_and_replaces_a_bad_one() {
        let app = router_with(RequestId::default());
        let kept = get(&app, "/", &[("x-request-id", "abc-123")]).await;
        assert_eq!(kept.headers()["x-request-id"], "abc-123");

        let replaced = get(&app, "/", &[("x-request-id", "has space")]).await;
        let id = replaced.headers()["x-request-id"].to_str().unwrap();
        assert!(Uuid::parse_str(id).is_ok(), "{id}");

        let generated = get(&app, "/", &[]).await;
        assert!(generated.headers().contains_key("x-request-id"));
    }
}
