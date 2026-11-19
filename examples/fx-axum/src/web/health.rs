//! `GET /health`.

use axum::{Router, routing::get};

use super::Controller;

/// Liveness probe.
#[derive(Default)]
pub struct HealthController;

#[fx::component]
impl HealthController {
    /// Constructor.
    #[fx::constructor]
    pub fn new() -> Self {
        Self
    }
}

#[fx::bind(group)]
impl Controller for HealthController {
    fn routes(&self) -> Router {
        Router::new().route("/health", get(|| async { "ok" }))
    }
}
