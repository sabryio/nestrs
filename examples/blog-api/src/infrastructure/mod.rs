//! **Infrastructure layer**: adapters that implement the ports.
//!
//! Persistence, token verification, configuration and start-up seeding. This is
//! where technology choices live; swapping one (in-memory to Postgres, static token
//! to JWT) means adding an adapter here and changing one `provides = ..` (or provider function) in `composition`.
//! Depends on `domain` and `application` (to implement their ports), never on
//! `presentation` or `composition`.

pub mod announcements;
pub mod auth;
pub mod config;
pub mod events;
pub mod persistence;
pub mod seed;
pub mod shared_state;
