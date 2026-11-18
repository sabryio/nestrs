use purlin_http::Interceptor;
use purlin_http::axum::extract::Request;
use purlin_http::axum::http::HeaderValue;
use purlin_http::axum::middleware::Next;
use purlin_http::axum::response::Response;
use tracing::Instrument;
use uuid::Uuid;

/// W3C Trace Context for the current request. Take it as a handler parameter (to
/// forward `traceparent` to downstream calls) once [`TraceContextInterceptor`] is installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceContext {
    /// 32 hex characters shared by every service touched by one user action.
    pub trace_id: String,
    /// 16 hex characters naming *this* service's span.
    pub span_id: String,
    /// The caller's span, when the request arrived with a `traceparent`.
    pub parent_id: Option<String>,
    /// Whether the caller asked for this trace to be recorded.
    pub sampled: bool,
}

extension_extractor!(TraceContext, "TraceContextInterceptor");

impl TraceContext {
    /// The `traceparent` header value for a call made from this span.
    pub fn traceparent(&self) -> String {
        format!(
            "00-{}-{}-{}",
            self.trace_id,
            self.span_id,
            if self.sampled { "01" } else { "00" }
        )
    }

    /// Parses a `traceparent` header (version 00, non-zero ids) and starts a child span.
    fn continue_from(header: &str) -> Option<Self> {
        let mut parts = header.trim().split('-');
        let (version, trace_id, parent_id, flags) = (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
        let hex =
            |s: &str, len: usize| s.len() == len && s.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase());
        let nonzero = |s: &str| s.bytes().any(|b| b != b'0');
        let valid = version == "00"
            && parts.next().is_none()
            && hex(trace_id, 32)
            && nonzero(trace_id)
            && hex(parent_id, 16)
            && nonzero(parent_id)
            && hex(flags, 2);
        valid.then(|| Self {
            trace_id: trace_id.to_owned(),
            span_id: new_span_id(),
            parent_id: Some(parent_id.to_owned()),
            sampled: u8::from_str_radix(flags, 16).is_ok_and(|f| f & 1 == 1),
        })
    }

    fn start() -> Self {
        Self {
            trace_id: Uuid::new_v4().simple().to_string(),
            span_id: new_span_id(),
            parent_id: None,
            sampled: true,
        }
    }
}

fn new_span_id() -> String {
    Uuid::new_v4().simple().to_string()[..16].to_owned()
}

/// Joins (or starts) a distributed trace by the W3C `traceparent` standard.
///
/// An incoming valid `traceparent` is continued: same trace id, a fresh span id for this
/// service, the caller's span recorded as the parent. Anything missing or malformed
/// starts a new trace instead of failing the request. The new `traceparent` goes on the
/// response, and `trace_id` / `span_id` become fields on the request's `tracing` span, so
/// every log line of the request can be joined with the other services' logs.
///
/// This propagates context; exporting spans to a backend is the job of an
/// OpenTelemetry layer on your `tracing` subscriber, which will pick these ids up.
#[derive(Debug, Clone, Copy, Default)]
pub struct TraceContextInterceptor;

impl Interceptor for TraceContextInterceptor {
    async fn intercept(&self, mut request: Request, next: Next) -> Response {
        let context = request
            .headers()
            .get("traceparent")
            .and_then(|value| value.to_str().ok())
            .and_then(TraceContext::continue_from)
            .unwrap_or_else(TraceContext::start);
        let span = tracing::info_span!("trace", trace_id = %context.trace_id, span_id = %context.span_id);
        let traceparent = HeaderValue::from_str(&context.traceparent()).expect("hex is header-safe");
        request.extensions_mut().insert(context);

        let mut response = next.run(request).instrument(span).await;
        response.headers_mut().insert("traceparent", traceparent);
        response
    }
}

#[cfg(test)]
mod tests {
    use crate::testing::{get, router_with};

    use super::*;

    const INCOMING: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

    #[tokio::test]
    async fn continues_an_incoming_trace_with_a_new_span() {
        let response = get(&router_with(TraceContextInterceptor), "/", &[("traceparent", INCOMING)]).await;
        let out = response.headers()["traceparent"].to_str().unwrap();
        assert!(out.starts_with("00-4bf92f3577b34da6a3ce929d0e0e4736-"), "{out}");
        assert!(!out.contains("00f067aa0ba902b7"), "a new span id, not the caller's");
        assert!(out.ends_with("-01"), "sampling is carried over");
    }

    #[tokio::test]
    async fn starts_a_new_trace_when_the_header_is_missing_or_malformed() {
        for header in [
            None,
            Some("garbage"),
            Some("00-00000000000000000000000000000000-00f067aa0ba902b7-01"),
            Some("01-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"),
            Some("00-4BF92F3577B34DA6A3CE929D0E0E4736-00f067aa0ba902b7-01"),
        ] {
            let headers: Vec<(&str, &str)> = header.map(|h| ("traceparent", h)).into_iter().collect();
            let response = get(&router_with(TraceContextInterceptor), "/", &headers).await;
            let out = response.headers()["traceparent"].to_str().unwrap();
            assert_eq!(out.len(), 55, "{out}");
            if let Some(h) = header {
                assert!(!out.contains(&h[3..35.min(h.len())]) || h == "garbage", "{h}");
            }
        }
    }

    #[test]
    fn parses_flags_and_parent() {
        let context = TraceContext::continue_from("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-00").unwrap();
        assert!(!context.sampled);
        assert_eq!(context.parent_id.as_deref(), Some("00f067aa0ba902b7"));
        assert!(context.traceparent().ends_with("-00"));
    }
}
