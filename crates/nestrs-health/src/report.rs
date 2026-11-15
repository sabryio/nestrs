//! What the probes return.

use nestrs::openapi::ToSchema;
use serde::Serialize;

/// Outcome of one check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Healthy.
    Up,
    /// Failed, timed out or panicked.
    Down,
}

/// One indicator's result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct CheckResult {
    /// The indicator's name.
    pub name: String,
    /// Whether it is healthy.
    pub status: Status,
    /// Whether its failure makes the service not ready.
    pub critical: bool,
    /// How long the check took, in milliseconds.
    pub latency_ms: u64,
    /// Why it failed; absent when healthy or when details are hidden.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Body of the liveness probe.
#[derive(Debug, Serialize, ToSchema)]
pub struct Liveness {
    /// Always `"ok"` while the process is serving.
    pub status: &'static str,
}

/// Body of a successful readiness probe: every critical check passed.
#[derive(Debug, Serialize, ToSchema)]
pub struct Readiness {
    /// `"ok"`, or `"degraded"` when only non-critical checks are down.
    pub status: &'static str,
    /// Every check's result.
    pub checks: Vec<CheckResult>,
}
