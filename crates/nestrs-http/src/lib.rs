//! # nestrs-http
//!
//! The Axum adapter. It plugs into `nestrs-core` through the [`Transport`] contract
//! and adds the HTTP-specific vocabulary:
//!
//! - [`HttpController`]: what `#[controller]` implements
//! - [`Guard`]: authorisation/pre-conditions that run before a handler
//! - [`ValidatedJson`]: JSON extractor that runs `validator` rules
//! - [`HttpError`]: one error type that renders a consistent JSON body
//! - [`App`]: bootstraps a module graph and serves it
//!
//! `axum` is re-exported so applications and generated code agree on the version.
//!
//! [`Transport`]: nestrs_core::Transport

#![forbid(unsafe_code)]

mod app;
mod auth;
mod auth_cache;
mod bearer;
mod controller;
mod error;
mod filter;
mod guard;
mod interceptor;
mod mount;
pub mod openapi;
mod path;
mod pipe;
mod rate_limit;
mod scope;
mod sse;
mod timeout;
mod validation;
mod ws;

pub use app::{App, AppBuilder, AppError};
pub use auth::{Principal, Requirement, authorized};
pub use auth_cache::{CacheConfig, CachedAuthenticator, cached};
pub use bearer::{AuthError, Authenticator, AuthenticatorPort, BearerAuth, OptionalBearerAuth, StaticTokens};
pub use controller::{
    HttpController, HttpRoutes, HttpTransport, assert_last_extractor, assert_parts_extractor, mount,
    with_default_status,
};
pub use error::HttpError;
pub use filter::ExceptionFilter;
pub use guard::{Guard, guarded, guarded_router};
pub use interceptor::{Interceptor, inline_interceptor, intercepted, intercepted_router};
pub use mount::Mount;
pub use openapi::ApiInfo;
pub use path::join_path;
pub use pipe::{Pipe, Piped};
pub use rate_limit::{Decision, KeyBy, MemoryStore, RateLimit, RateLimitStore, StoreError};
pub use scope::{HttpModule, Inject, RequestInfo};
pub use sse::{Event, EventStream, LastEventId, ShuttingDown};
pub use timeout::Timeout;
pub use validation::ValidatedJson;
pub use ws::{Channel, Ws, WsError};

pub use axum;
pub use axum::http::request::Parts;
pub use futures_util::stream::{self, Stream, StreamExt};
pub use utoipa;
