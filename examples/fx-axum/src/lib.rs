//! A plain [`axum`] service wired with `rust-fx` only.
//!
//! ```text
//! src/
//!   domain.rs      what the service is about: Greeting, the Clock and GreetingStore ports,
//!                  and GreetingService. Knows neither axum nor any adapter.
//!   infra.rs       adapters for the ports (system clock, in-memory store).
//!   web/mod.rs     the Controller port: "something that contributes axum routes".
//!   web/greetings.rs, web/health.rs   two controllers.
//!   web/server.rs  ServerConfig and the server component (bind, serve, graceful stop).
//!   modules.rs     the fx modules, and `app()`, the composition root.
//! ```
//!
//! Dependencies point inwards: `web` and `infra` depend on `domain`; `domain` depends on
//! nothing. Adding an endpoint group means adding a controller and listing it in a module;
//! nothing else changes.

#![forbid(unsafe_code)]

pub mod domain;
pub mod infra;
pub mod modules;
pub mod web;

pub use modules::app;
