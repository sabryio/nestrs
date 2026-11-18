//! Configuration for [`Telemetry`](crate::Telemetry).

use std::time::Duration;

use crate::OtelError;

/// How traces are identified and exported.
///
/// Plain data with defaults, so it can be a field of an application's own config struct and
/// filled from the environment like the rest of it.
#[derive(Debug, Clone, PartialEq)]
pub struct OtelConfig {
    /// `service.name` on every span. The one attribute every backend needs.
    pub service_name: String,
    /// `service.version`, when known.
    pub service_version: Option<String>,
    /// Full OTLP/HTTP traces URL, including the path (`http://collector:4318/v1/traces`).
    /// `None` keeps tracing local: spans are created and propagated but not exported.
    pub endpoint: Option<String>,
    /// Fraction of *new* traces recorded, `0.0..=1.0`. A trace that arrives already sampled
    /// (or not) keeps the caller's decision, so a distributed trace is never half-recorded.
    pub sample_ratio: f64,
    /// How long one export request may take.
    pub export_timeout: Duration,
    /// Extra resource attributes (`deployment.environment`, `service.namespace`, ...).
    pub resource_attributes: Vec<(String, String)>,
}

impl OtelConfig {
    /// Defaults for a service called `service_name`: sample everything, no export.
    pub fn new(service_name: impl Into<String>) -> Self {
        Self {
            service_name: service_name.into(),
            service_version: None,
            endpoint: None,
            sample_ratio: 1.0,
            export_timeout: Duration::from_secs(10),
            resource_attributes: Vec::new(),
        }
    }

    /// Export to this OTLP/HTTP traces URL.
    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = Some(endpoint.into());
        self
    }

    /// Record this fraction of new traces.
    pub fn with_sample_ratio(mut self, ratio: f64) -> Self {
        self.sample_ratio = ratio;
        self
    }

    /// Sets `service.version`.
    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.service_version = Some(version.into());
        self
    }

    /// Adds a resource attribute.
    pub fn with_attribute(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.resource_attributes.push((key.into(), value.into()));
        self
    }

    pub(crate) fn validate(&self) -> Result<(), OtelError> {
        if (0.0..=1.0).contains(&self.sample_ratio) {
            Ok(())
        } else {
            Err(OtelError::SampleRatio(self.sample_ratio))
        }
    }
}
