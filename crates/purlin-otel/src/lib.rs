//! OpenTelemetry for purlin.
//!
//! The framework crates emit plain [`tracing`] spans and know nothing about OpenTelemetry. This
//! crate is the adapter:
//!
//! * [`Telemetry`] starts the pipeline (OTLP/HTTP export, sampling, resource) and gives you a
//!   `tracing-subscriber` layer.
//! * [`HttpTracing`] is an interceptor that opens the server span and continues the caller's trace.
//! * [`OtelPropagation`] carries the trace through queued messages, so a worker's span is a child
//!   of the request that enqueued the work.
//!
//! ```ignore
//! let telemetry = Telemetry::init(&OtelConfig::new("blog").with_endpoint("http://otel:4318/v1/traces"))?;
//! tracing_subscriber::registry().with(telemetry.layer()).with(fmt::layer()).init();
//! // JobsConfig { propagation: Some(Arc::new(OtelPropagation::new())), .. }
//! // #[interceptor(HttpTracing)] on controllers
//! telemetry.shutdown(); // on the way out
//! ```
//!
//! One gap is deliberate: a transactional-outbox row does not store trace context, so the
//! relay's `outbox.publish` span starts a new trace; from the queue onwards the trace is linked.

mod config;
mod error;
mod http;
mod propagation;
mod telemetry;

pub use config::OtelConfig;
pub use error::OtelError;
pub use http::HttpTracing;
pub use propagation::OtelPropagation;
pub use telemetry::Telemetry;
