//! Example application for nestrs: a small blog API in clean architecture.
//!
//! ```text
//! domain/          entities, value objects, repository ports        (depends on nothing)
//! application/     use cases, the token-verifier port               (-> domain)
//! infrastructure/  adapters: persistence, auth, config, seeding     (-> domain, application)
//! presentation/    HTTP controllers, DTOs, guard, error mapping     (-> application, domain)
//! composition/     nestrs modules wiring ports to adapters          (-> everything)
//! ```
//!
//! Dependencies point inward only; `tests/architecture.rs` fails the build if an
//! import breaks that rule. [`AppModule`] is the composition root.

pub mod application;
pub mod composition;
pub mod domain;
pub mod infrastructure;
pub mod presentation;

pub use composition::AppModule;
