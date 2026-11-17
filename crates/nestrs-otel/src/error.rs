//! Errors from setting telemetry up.

/// Telemetry could not be started. Only configuration and exporter construction can fail;
/// exporting itself never errors into the request path.
#[derive(Debug, thiserror::Error)]
pub enum OtelError {
    /// The OTLP exporter could not be built (bad endpoint, TLS or client setup).
    #[error("could not build the OTLP exporter: {0}")]
    Exporter(String),
    /// `sample_ratio` was outside `0.0..=1.0`.
    #[error("sample_ratio must be between 0 and 1, got {0}")]
    SampleRatio(f64),
}
