//! Shared by the test binaries: an in-memory exporter wired into a subscriber.
#![allow(dead_code)]

use opentelemetry_sdk::trace::{InMemorySpanExporter, SpanData};
use purlin_otel::{OtelConfig, Telemetry};
use tracing_subscriber::layer::SubscriberExt;

pub struct Harness {
    pub exporter: InMemorySpanExporter,
    _guard: tracing::subscriber::DefaultGuard,
    pub telemetry: Telemetry,
}

impl Harness {
    pub fn new() -> Self {
        let exporter = InMemorySpanExporter::default();
        let telemetry = Telemetry::with_exporter(&OtelConfig::new("test"), exporter.clone()).unwrap();
        let subscriber = tracing_subscriber::registry().with(telemetry.layer());
        Self {
            exporter,
            _guard: tracing::subscriber::set_default(subscriber),
            telemetry,
        }
    }

    pub fn spans(&self, name: &str) -> Vec<SpanData> {
        self.exporter
            .get_finished_spans()
            .unwrap()
            .into_iter()
            .filter(|span| span.name == name)
            .collect()
    }
}

pub fn attribute(span: &SpanData, key: &str) -> Option<String> {
    span.attributes
        .iter()
        .find(|kv| kv.key.as_str() == key)
        .map(|kv| kv.value.to_string())
}
