//! Owns the OpenTelemetry tracer provider and bridges it to `tracing`.

use opentelemetry::trace::TracerProvider as _;
use opentelemetry::{KeyValue, global};
use opentelemetry_otlp::{SpanExporter as OtlpExporter, WithExportConfig};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::trace::{Sampler, SdkTracer, SdkTracerProvider, SpanExporter};
use tracing::Subscriber;
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::registry::LookupSpan;

use crate::{OtelConfig, OtelError};

/// A running OpenTelemetry pipeline.
///
/// Add [`layer`](Self::layer) to your `tracing` subscriber: every `tracing` span (including the
/// framework's `http.request`, `job.run`, `queue.handle` and `outbox.publish`) then becomes an
/// OpenTelemetry span. Call [`shutdown`](Self::shutdown) before exit so buffered spans are sent.
pub struct Telemetry {
    provider: SdkTracerProvider,
    tracer: SdkTracer,
}

fn resource(config: &OtelConfig) -> Resource {
    let mut builder = Resource::builder().with_service_name(config.service_name.clone());
    if let Some(version) = &config.service_version {
        builder = builder.with_attribute(KeyValue::new("service.version", version.clone()));
    }
    for (key, value) in &config.resource_attributes {
        builder = builder.with_attribute(KeyValue::new(key.clone(), value.clone()));
    }
    builder.build()
}

fn sampler(config: &OtelConfig) -> Sampler {
    Sampler::ParentBased(Box::new(Sampler::TraceIdRatioBased(config.sample_ratio)))
}

impl Telemetry {
    /// Starts the pipeline: exports over OTLP/HTTP when `config.endpoint` is set, otherwise
    /// only creates and propagates spans. Also installs the W3C propagator and the tracer
    /// provider globally, for code that uses the OpenTelemetry API directly.
    pub fn init(config: &OtelConfig) -> Result<Self, OtelError> {
        config.validate()?;
        let mut builder = SdkTracerProvider::builder()
            .with_resource(resource(config))
            .with_sampler(sampler(config));
        if let Some(endpoint) = &config.endpoint {
            let exporter = OtlpExporter::builder()
                .with_http()
                .with_endpoint(endpoint.clone())
                .with_timeout(config.export_timeout)
                .build()
                .map_err(|error| OtelError::Exporter(error.to_string()))?;
            builder = builder.with_batch_exporter(exporter);
        }
        let telemetry = Self::from_provider(builder.build());
        global::set_text_map_propagator(TraceContextPropagator::new());
        global::set_tracer_provider(telemetry.provider.clone());
        Ok(telemetry)
    }

    /// Like [`init`](Self::init) but exports synchronously to `exporter` and touches no global
    /// state. For tests (with an in-memory exporter) and for custom exporters.
    pub fn with_exporter(config: &OtelConfig, exporter: impl SpanExporter + 'static) -> Result<Self, OtelError> {
        config.validate()?;
        let provider = SdkTracerProvider::builder()
            .with_resource(resource(config))
            .with_sampler(sampler(config))
            .with_simple_exporter(exporter)
            .build();
        Ok(Self::from_provider(provider))
    }

    fn from_provider(provider: SdkTracerProvider) -> Self {
        let tracer = provider.tracer("nestrs");
        Self { provider, tracer }
    }

    /// The `tracing-subscriber` layer that turns `tracing` spans into OpenTelemetry spans.
    pub fn layer<S>(&self) -> OpenTelemetryLayer<S, SdkTracer>
    where
        S: Subscriber + for<'span> LookupSpan<'span>,
    {
        tracing_opentelemetry::layer().with_tracer(self.tracer.clone())
    }

    /// Flushes buffered spans and stops exporting. Safe to call more than once.
    pub fn shutdown(&self) {
        if let Err(error) = self.provider.shutdown() {
            tracing::warn!(%error, "telemetry shutdown did not complete cleanly");
        }
    }
}
