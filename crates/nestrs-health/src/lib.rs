//! # nestrs-health
//!
//! Liveness and readiness probes.
//!
//! ```ignore
//! #[module(imports = [HealthModule], ..)]
//! struct AppModule;
//!
//! // anywhere, in any module: add a check to the readiness set
//! #[injectable(contributes = HealthIndicatorPort)]
//! struct DatabaseIndicator { .. }
//! ```
//!
//! * `GET /health/live`: always 200 while the process serves. Wire it to the
//!   orchestrator's *liveness* probe (restart on failure).
//! * `GET /health/ready`: 200 when every critical indicator is up; otherwise, or once
//!   shutdown begins, 503 so the load balancer drains traffic *before* the server stops.
//!
//! ## Design
//!
//! * [`HealthIndicator`] is a port; features contribute implementations to a
//!   multi-binding ([`Multi`](nestrs::Multi)), so adding a check never edits this crate (OCP).
//! * [`HealthChecker`] runs them; the controller only maps results to HTTP (SRP).
//! * The controller depends on the checker, the checker on the set, never on any
//!   concrete indicator (DIP).

#![forbid(unsafe_code)]

mod checker;
mod controller;
mod indicator;
mod module;
mod report;

pub use checker::{HealthChecker, HealthConfig};
pub use controller::HealthController;
pub use indicator::{HealthIndicator, HealthIndicatorPort, Unhealthy};
pub use module::HealthModule;
pub use report::{CheckResult, Liveness, Readiness, Status};
