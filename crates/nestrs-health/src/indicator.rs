//! The port every health check implements.

use nestrs::prelude::*;

/// Why a check failed. The message is shown to whoever can call the readiness
/// endpoint (unless [`HealthConfig::expose_details`](crate::HealthConfig) is off), so
/// keep secrets, hostnames and connection strings out of it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct Unhealthy(pub String);

impl Unhealthy {
    /// A failure with a human-readable reason.
    pub fn new(reason: impl Into<String>) -> Self {
        Self(reason.into())
    }
}

/// Port: one dependency's health, such as the database, a queue or a downstream API.
///
/// Implementations are *contributed* to the readiness set from any module, so a
/// feature module owns its own check and the health module never learns about it:
///
/// ```ignore
/// #[injectable(contributes = HealthIndicatorPort)]
/// pub struct DatabaseIndicator { db: DatabaseConnection }
///
/// #[async_trait]
/// impl HealthIndicator for DatabaseIndicator {
///     fn name(&self) -> &str { "database" }
///     async fn check(&self) -> Result<(), Unhealthy> {
///         self.db.ping().await.map_err(|e| Unhealthy::new(e.to_string()))
///     }
/// }
/// ```
///
/// Contract: `check` should be cheap and side-effect free, and may be called by many
/// probes at once. It is cancelled when it exceeds the configured timeout, so it must
/// be cancel-safe; a panic is reported as a failure, not a crash.
#[port]
pub trait HealthIndicator {
    /// A stable, unique label, shown in the report (`"database"`).
    fn name(&self) -> &str;

    /// Whether this indicator applies in this deployment. A disabled one is left out of the
    /// report entirely (not shown as healthy), so an optional dependency that is not configured
    /// does not clutter the answer. Defaults to `true`.
    fn enabled(&self) -> bool {
        true
    }

    /// Whether a failure makes the service *not ready*. A non-critical check (a
    /// recommendation service, a cache) is still reported, but only degrades the
    /// answer. Defaults to `true`.
    fn critical(&self) -> bool {
        true
    }

    /// Verifies the dependency. `Ok` means healthy.
    async fn check(&self) -> Result<(), Unhealthy>;
}
