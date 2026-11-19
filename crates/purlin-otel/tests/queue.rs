//! Telemetry setup and queue propagation, headless. Spans are checked against an in-memory
//! exporter, so no collector is needed.

mod common;

use common::{Harness, attribute};

use std::sync::Arc;
use std::time::Duration;

use opentelemetry::trace::{SpanKind, Status};
use opentelemetry_sdk::trace::InMemorySpanExporter;
use purlin_jobs::{HandlerError, JobsConfig, JobsModule, Queue, QueueHandler, QueueHandlerPort};
use purlin_otel::{OtelConfig, OtelError, OtelPropagation, Telemetry};
use purlin_testing::TestApp;
use tracing::Instrument;

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
