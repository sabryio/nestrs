//! Runs the indicators.

use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration;

use futures_util::FutureExt;
use futures_util::future::join_all;
use purlin::prelude::*;
use tokio::time::Instant;

use crate::indicator::{HealthIndicator, Unhealthy};
use crate::report::{CheckResult, Status};

/// How the probes behave.
#[derive(Debug, Clone)]
pub struct HealthConfig {
    /// Longest a single indicator may take; slower ones count as down.
    pub timeout: Duration,
    /// Include failure reasons in responses. Turn it off when the readiness endpoint
    /// is reachable by untrusted callers.
    pub expose_details: bool,
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(2),
            expose_details: true,
        }
    }
}

/// Runs every contributed [`HealthIndicator`] concurrently, each under a deadline.
///
/// One slow or broken indicator cannot hold up, hide or crash the others.
#[injectable]
pub struct HealthChecker {
    indicators: Arc<Multi<dyn HealthIndicator>>,
    config: Arc<HealthConfig>,
}

impl HealthChecker {
    /// Runs all checks and returns their results in registration order.
    pub async fn run(&self) -> Vec<CheckResult> {
        join_all(
            self.indicators
                .iter()
                .filter(|indicator| indicator.enabled())
                .map(|indicator| self.run_one(indicator.as_ref())),
        )
        .await
    }

    async fn run_one(&self, indicator: &dyn HealthIndicator) -> CheckResult {
        let started = Instant::now();
        let outcome = tokio::time::timeout(
            self.config.timeout,
            // A panicking indicator is a failed check, not a failed probe.
            AssertUnwindSafe(indicator.check()).catch_unwind(),
        )
        .await;
        let failure = match outcome {
            Ok(Ok(Ok(()))) => None,
            Ok(Ok(Err(Unhealthy(reason)))) => Some(reason),
            Ok(Err(_)) => Some("the check panicked".to_owned()),
            Err(_) => Some(format!("timed out after {:?}", self.config.timeout)),
        };
        if let Some(reason) = &failure {
            tracing::warn!(check = indicator.name(), %reason, "health check failed");
        }
        CheckResult {
            name: indicator.name().to_owned(),
            status: if failure.is_none() { Status::Up } else { Status::Down },
            critical: indicator.critical(),
            latency_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            error: failure.filter(|_| self.config.expose_details),
        }
    }
}
