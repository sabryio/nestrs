//! The probe endpoints.

use std::sync::Arc;

use nestrs::http::ShuttingDown;
use nestrs::prelude::*;
use serde_json::json;

use crate::checker::HealthChecker;
use crate::report::{Liveness, Readiness, Status};

/// `GET /health/live` and `GET /health/ready`.
#[injectable]
pub struct HealthController {
    checker: Arc<HealthChecker>,
}

#[controller("/health")]
impl HealthController {
    /// Liveness: the process is up and serving. Touches no dependency, so a failing
    /// database never gets the pod restarted.
    #[get("/live")]
    async fn live(&self) -> Json<Liveness> {
        Json(Liveness { status: "ok" })
    }

    /// Readiness: every critical dependency is healthy and the app is not shutting
    /// down. Answers 503 otherwise, with the per-check results in `error.details`.
    #[get("/ready")]
    #[response(503, "Not ready: a critical check is failing, or the app is draining")]
    async fn ready(&self, shutting_down: ShuttingDown) -> Result<Json<Readiness>, HttpError> {
        // Draining skips the checks: the answer is already known and a closing
        // service should not poke its dependencies.
        if shutting_down.is_set() {
            return Err(HttpError::unavailable(
                "shutting down",
                Some(json!({ "status": "draining", "checks": [] })),
            ));
        }

        let checks = self.checker.run().await;
        let failing = checks.iter().filter(|c| c.status == Status::Down && c.critical).count();
        if failing > 0 {
            return Err(HttpError::unavailable(
                format!("not ready: {failing} critical check(s) failing"),
                Some(json!({ "status": "unavailable", "checks": checks })),
            ));
        }
        let degraded = checks.iter().any(|c| c.status == Status::Down);
        Ok(Json(Readiness {
            status: if degraded { "degraded" } else { "ok" },
            checks,
        }))
    }
}
