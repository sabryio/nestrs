//! The HTTP server span.

use nestrs_http::Interceptor;
use nestrs_http::axum::extract::{MatchedPath, Request};
use nestrs_http::axum::http::HeaderValue;
use nestrs_http::axum::middleware::Next;
use nestrs_http::axum::response::Response;
use opentelemetry::trace::TraceContextExt as _;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use tracing::Instrument;
use tracing_opentelemetry::OpenTelemetrySpanExt;

use crate::propagation::{extract, inject};

/// Wraps each request in a server span named `METHOD /route/{template}` and continues the
/// caller's trace when the request carries a `traceparent`.
///
/// Attributes follow the OpenTelemetry HTTP semantic conventions (`http.request.method`,
/// `http.route`, `http.response.status_code`); a 5xx response marks the span as an error. The
/// route is the *template*, never the raw path, so span names stay low-cardinality. The
/// response gets a `traceparent` header so callers can find the trace.
///
/// ```ignore
/// #[controller("/posts")]
/// #[interceptor(HttpTracing)]
/// impl PostsController { /* ... */ }
/// ```
#[derive(Debug, Default, Clone)]
pub struct HttpTracing {
    propagator: TraceContextPropagator,
}

impl HttpTracing {
    /// A tracing interceptor with the W3C propagator.
    pub fn new() -> Self {
        Self::default()
    }
}

impl Interceptor for HttpTracing {
    async fn intercept(&self, request: Request, next: Next) -> Response {
        let method = request.method().as_str().to_owned();
        let route = request
            .extensions()
            .get::<MatchedPath>()
            .map_or("unmatched", MatchedPath::as_str)
            .to_owned();
        let span = tracing::info_span!(
            "http.request",
            otel.name = %format!("{method} {route}"),
            otel.kind = "server",
            otel.status_code = tracing::field::Empty,
            http.request.method = %method,
            http.route = %route,
            http.response.status_code = tracing::field::Empty,
        );
        if let Some(incoming) = request.headers().get("traceparent").and_then(|v| v.to_str().ok()) {
            let context = extract(&self.propagator, incoming);
            if context.span().span_context().is_valid() {
                let _ = span.set_parent(context);
            }
        }

        let mut response = next.run(request).instrument(span.clone()).await;

        let status = response.status();
        span.record("http.response.status_code", status.as_u16());
        if status.is_server_error() {
            span.record("otel.status_code", "ERROR");
        }
        if let Some(value) = inject(&self.propagator, &span).and_then(|v| HeaderValue::from_str(&v).ok()) {
            response.headers_mut().insert("traceparent", value);
        }
        response
    }
}
