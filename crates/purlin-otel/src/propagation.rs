//! W3C Trace Context in and out of `tracing` spans.

use std::collections::HashMap;

use opentelemetry::Context;
use opentelemetry::propagation::TextMapPropagator;
use opentelemetry::trace::TraceContextExt;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use purlin_jobs::TracePropagation;
use tracing_opentelemetry::OpenTelemetrySpanExt;

const TRACEPARENT: &str = "traceparent";

/// The `traceparent` of `span`, if it is a recorded OpenTelemetry span.
pub(crate) fn inject(propagator: &TraceContextPropagator, span: &tracing::Span) -> Option<String> {
    let context = span.context();
    if !context.span().span_context().is_valid() {
        return None;
    }
    let mut carrier = HashMap::new();
    propagator.inject_context(&context, &mut carrier);
    carrier.remove(TRACEPARENT)
}

/// The remote context carried by a `traceparent` value.
pub(crate) fn extract(propagator: &TraceContextPropagator, traceparent: &str) -> Context {
    let carrier = HashMap::from([(TRACEPARENT.to_owned(), traceparent.to_owned())]);
    propagator.extract(&carrier)
}

/// Carries trace context through queued messages (implements `purlin-jobs`' port).
///
/// Install it on the jobs configuration:
/// `JobsConfig { propagation: Some(Arc::new(OtelPropagation::new())), ..Default::default() }`.
#[derive(Debug, Default, Clone)]
pub struct OtelPropagation {
    propagator: TraceContextPropagator,
}

impl OtelPropagation {
    /// A W3C Trace Context propagator.
    pub fn new() -> Self {
        Self::default()
    }
}

impl TracePropagation for OtelPropagation {
    fn current(&self) -> Option<String> {
        inject(&self.propagator, &tracing::Span::current())
    }

    fn adopt(&self, span: &tracing::Span, carrier: &str) {
        let context = extract(&self.propagator, carrier);
        if context.span().span_context().is_valid() {
            // Fails only when the span is not an OpenTelemetry span (no layer installed);
            // then there is nothing to link and the span simply starts its own trace.
            let _ = span.set_parent(context);
        }
    }
}
