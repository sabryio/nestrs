//! HTTP server spans (feature `http`). Spans are checked against an in-memory exporter.

mod common;

use std::sync::Arc;

use common::{Harness, attribute};
use opentelemetry::trace::{SpanKind, Status};
use purlin_http::axum::Router;
use purlin_http::axum::body::Body;
use purlin_http::axum::http::{Request, StatusCode};
use purlin_http::axum::routing::get;
use purlin_http::intercepted;
use purlin_otel::HttpTracing;
use tower::ServiceExt;

const INCOMING: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
const INCOMING_TRACE: &str = "4bf92f3577b34da6a3ce929d0e0e4736";

fn app() -> Router {
    let tracing = Arc::new(HttpTracing::new());
    Router::new()
        .route("/items/{id}", intercepted(get(|| async { "ok" }), tracing.clone()))
        .route(
            "/boom",
            intercepted(get(|| async { StatusCode::INTERNAL_SERVER_ERROR }), tracing.clone()),
        )
        .route(
            "/missing",
            intercepted(get(|| async { StatusCode::NOT_FOUND }), tracing),
        )
}

async fn call(app: &Router, path: &str, traceparent: Option<&str>) -> purlin_http::axum::response::Response {
    let mut request = Request::get(path);
    if let Some(value) = traceparent {
        request = request.header("traceparent", value);
    }
    app.clone().oneshot(request.body(Body::empty()).unwrap()).await.unwrap()
}

#[tokio::test]
async fn a_request_becomes_a_server_span_named_after_the_route_template() {
    let harness = Harness::new();
    call(&app(), "/items/42", None).await;

    let spans = harness.spans("GET /items/{id}");
    assert_eq!(spans.len(), 1, "{:?}", harness.exporter.get_finished_spans());
    let span = &spans[0];
    assert_eq!(span.span_kind, SpanKind::Server);
    assert_eq!(attribute(span, "http.route").as_deref(), Some("/items/{id}"));
    assert_eq!(attribute(span, "http.request.method").as_deref(), Some("GET"));
    assert_eq!(attribute(span, "http.response.status_code").as_deref(), Some("200"));
    assert_eq!(span.status, Status::Unset);
}

#[tokio::test]
async fn server_errors_mark_the_span_but_client_errors_do_not() {
    let harness = Harness::new();
    let app = app();
    call(&app, "/boom", None).await;
    call(&app, "/missing", None).await;

    assert!(matches!(harness.spans("GET /boom")[0].status, Status::Error { .. }));
    assert_eq!(harness.spans("GET /missing")[0].status, Status::Unset);
}

#[tokio::test]
async fn an_incoming_traceparent_is_continued_and_echoed() {
    let harness = Harness::new();
    let response = call(&app(), "/items/1", Some(INCOMING)).await;

    let span = &harness.spans("GET /items/{id}")[0];
    assert_eq!(span.span_context.trace_id().to_string(), INCOMING_TRACE);
    assert_eq!(span.parent_span_id.to_string(), "00f067aa0ba902b7");

    let echoed = response.headers()["traceparent"].to_str().unwrap();
    assert!(echoed.starts_with(&format!("00-{INCOMING_TRACE}-")), "{echoed}");
    assert!(
        echoed.contains(&span.span_context.span_id().to_string()),
        "the response names this span"
    );
}

#[tokio::test]
async fn without_or_with_a_bad_header_a_fresh_trace_starts() {
    let harness = Harness::new();
    let app = app();
    call(&app, "/items/1", None).await;
    call(&app, "/items/2", Some("garbage")).await;

    let spans = harness.spans("GET /items/{id}");
    assert_eq!(spans.len(), 2);
    for span in &spans {
        assert_eq!(span.parent_span_id, opentelemetry::trace::SpanId::INVALID);
        assert_ne!(span.span_context.trace_id().to_string(), INCOMING_TRACE);
    }
    assert_ne!(spans[0].span_context.trace_id(), spans[1].span_context.trace_id());
}

#[tokio::test]
async fn an_unsampled_parent_is_respected() {
    let harness = Harness::new();
    let response = call(
        &app(),
        "/items/1",
        Some("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-00"),
    )
    .await;
    assert!(
        harness.spans("GET /items/{id}").is_empty(),
        "the caller said not to record"
    );
    assert_eq!(response.status(), StatusCode::OK);
}
