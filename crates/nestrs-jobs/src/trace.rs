//! Carrying trace context across the queue, so a handler's span continues the producer's trace.

use std::sync::Arc;

/// Port: how a trace context travels with a queued message.
///
/// The queue stores an opaque W3C `traceparent` string next to the payload. This port is the
/// only thing that knows how to produce and consume one, so this crate stays free of any
/// tracing backend: `nestrs-otel` implements it with OpenTelemetry, and without one the queue
/// carries no context and nothing else changes.
pub trait TracePropagation: Send + Sync + 'static {
    /// The context of the span that is current right now, if there is one worth propagating.
    fn current(&self) -> Option<String>;

    /// Makes `span` a child of the remote context `carrier` (a value from [`current`](Self::current)).
    fn adopt(&self, span: &tracing::Span, carrier: &str);
}

/// Shared handle used in [`JobsConfig`](crate::JobsConfig).
pub type TracePropagationPort = Arc<dyn TracePropagation>;
