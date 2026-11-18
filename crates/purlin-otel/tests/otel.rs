//! Spans are checked against an in-memory exporter, so no collector is needed.

use std::sync::Arc;
use std::time::Duration;

use opentelemetry::trace::{SpanKind, Status};
use opentelemetry_sdk::trace::{InMemorySpanExporter, SpanData};
use purlin_http::axum::Router;
use purlin_http::axum::body::Body;
use purlin_http::axum::http::{Request, StatusCode};
use purlin_http::axum::routing::get;
use purlin_http::intercepted;
use purlin_jobs::{HandlerError, JobsConfig, JobsModule, Queue, QueueHandler, QueueHandlerPort};
use purlin_otel::{HttpTracing, OtelConfig, OtelError, OtelPropagation, Telemetry};
use purlin_testing::TestApp;
use tower::ServiceExt;
use tracing::Instrument;
use tracing_subscriber::layer::SubscriberExt;

const INCOMING: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
const INCOMING_TRACE: &str = "4bf92f3577b34da6a3ce929d0e0e4736";

struct Harness {
    exporter: InMemorySpanExporter,
    _guard: tracing::subscriber::DefaultGuard,
    telemetry: Telemetry,
}

impl Harness {
    fn new() -> Self {
        let exporter = InMemorySpanExporter::default();
        let telemetry = Telemetry::with_exporter(&OtelConfig::new("test"), exporter.clone()).unwrap();
        let subscriber = tracing_subscriber::registry().with(telemetry.layer());
        Self {
            exporter,
            _guard: tracing::subscriber::set_default(subscriber),
            telemetry,
        }
    }

    fn spans(&self, name: &str) -> Vec<SpanData> {
        self.exporter
            .get_finished_spans()
            .unwrap()
            .into_iter()
            .filter(|span| span.name == name)
            .collect()
    }
}

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

fn attribute(span: &SpanData, key: &str) -> Option<String> {
    span.attributes
        .iter()
        .find(|kv| kv.key.as_str() == key)
        .map(|kv| kv.value.to_string())
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

#[test]
fn an_invalid_sample_ratio_is_rejected() {
    let error = Telemetry::with_exporter(
        &OtelConfig::new("x").with_sample_ratio(1.5),
        InMemorySpanExporter::default(),
    )
    .err()
    .unwrap();
    assert!(matches!(error, OtelError::SampleRatio(r) if r == 1.5));
}

mod queue {
    use super::*;
    use purlin::prelude::*;
    use purlin_jobs::TracePropagation;

    #[injectable(contributes = QueueHandlerPort)]
    struct Worker;

    #[async_trait]
    impl QueueHandler for Worker {
        fn queue(&self) -> &str {
            "work"
        }
        fn max_attempts(&self) -> u32 {
            1
        }
        async fn handle(&self, payload: &str) -> Result<(), HandlerError> {
            if payload.contains("fail") {
                Err(HandlerError::Fatal("nope".into()))
            } else {
                Ok(())
            }
        }
    }

    #[module(imports = [JobsModule::in_memory()], providers = [Worker])]
    struct AppModule;

    async fn start() -> TestApp {
        TestApp::builder(AppModule)
            .override_value(JobsConfig {
                propagation: Some(Arc::new(OtelPropagation::new())),
                poll: Duration::from_millis(50),
                ..JobsConfig::default()
            })
            .start()
            .await
    }

    async fn settle() {
        for _ in 0..30 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test(start_paused = true)]
    async fn the_consumer_span_continues_the_producers_trace() {
        let harness = Harness::new();
        let app = start().await;
        let queue = app.resolve::<Queue>();

        let producer = tracing::info_span!("producer");
        queue
            .enqueue("work", &"hello")
            .instrument(producer.clone())
            .await
            .unwrap();
        drop(producer);
        settle().await;

        let producer = &harness.spans("producer")[0];
        let consumer = &harness.spans("work process")[0];
        assert_eq!(consumer.span_context.trace_id(), producer.span_context.trace_id());
        assert_eq!(consumer.parent_span_id, producer.span_context.span_id());
        assert_eq!(consumer.span_kind, SpanKind::Consumer);
        assert_eq!(
            attribute(consumer, "messaging.destination.name").as_deref(),
            Some("work")
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_failing_handler_marks_its_span() {
        let harness = Harness::new();
        let app = start().await;
        app.resolve::<Queue>().enqueue("work", &"fail").await.unwrap();
        settle().await;

        let consumer = &harness.spans("work process")[0];
        assert!(matches!(consumer.status, Status::Error { .. }), "{consumer:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn without_a_propagator_the_consumer_starts_its_own_trace() {
        let harness = Harness::new();
        let app = TestApp::builder(AppModule).start().await;
        let producer = tracing::info_span!("producer");
        app.resolve::<Queue>()
            .enqueue("work", &"hello")
            .instrument(producer.clone())
            .await
            .unwrap();
        drop(producer);
        settle().await;

        let producer = &harness.spans("producer")[0];
        let consumer = &harness.spans("work process")[0];
        assert_ne!(consumer.span_context.trace_id(), producer.span_context.trace_id());
    }

    #[test]
    fn no_span_means_nothing_to_propagate() {
        // No subscriber at all: `current` must not invent a context.
        assert_eq!(OtelPropagation::new().current(), None);
    }
}

#[test]
fn shutdown_twice_is_harmless() {
    let harness = Harness::new();
    harness.telemetry.shutdown();
    harness.telemetry.shutdown();
}
