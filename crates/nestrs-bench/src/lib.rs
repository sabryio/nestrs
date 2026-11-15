//! Apps used by the benchmarks and the soak test, so both measure the same thing.
//!
//! * [`overhead`]: the same `GET /hello` implemented as plain axum and as nestrs, with
//!   each feature (injection, guard, interceptors) added one at a time, so the cost of
//!   every layer shows up as the difference between two neighbouring rows.
//! * [`soak`]: a service with every protective interceptor on its own route, plus the
//!   counters the soak test reads to check invariants.

#![forbid(unsafe_code)]

pub mod overhead;
pub mod soak;
