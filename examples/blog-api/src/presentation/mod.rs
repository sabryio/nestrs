//! **Presentation layer**: how the application is exposed to the outside world.
//!
//! Only HTTP here. Controllers translate requests into use-case commands and results
//! into responses; DTOs are the wire format; this layer owns the single mapping from
//! application errors to HTTP errors. Depends on `application` and `domain`, never
//! on `infrastructure`: a controller cannot name a concrete adapter.

pub mod http;
