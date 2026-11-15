//! # nestrs-kit
//!
//! Reusable building blocks that every nestrs service ends up writing, packaged once:
//!
//! | Piece | What it is |
//! |---|---|
//! | [`TimingInterceptor`] | `x-response-time-ms` response header |
//! | [`ErrorLogFilter`] | logs 5xx failures with method, path and request id; never changes the response |
//! | [`RequestContext`] | request-scoped provider: the caller's `x-request-id` or a generated one |
//! | [`Pagination`], [`PaginationPipe`] | validated `?limit=&offset=`, documented in OpenAPI |
//! | [`KitModule`] | provides and exports the above |
//! | [`Service`] | the `main`: logging, prefix, OpenAPI (`--print-openapi`), global timeout and rate limit, tracing, graceful shutdown |
//!
//! None of it knows about any application's domain: an app keeps its own entities,
//! use cases and adapters, and uses the kit for the plumbing around them.
//!
//! ```ignore
//! #[module(imports = [KitModule, /* feature modules */])]
//! struct AppModule;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), AppError> {
//!     Service::new("My API", env!("CARGO_PKG_VERSION"))
//!         .prefix("/api")
//!         .timeout(Duration::from_secs(30))
//!         .run(AppModule, |c| Ok(c.resolve::<AppConfig>()?.bind_addr.clone()))
//!         .await
//! }
//! ```

#![forbid(unsafe_code)]

mod error_log;
mod module;
mod pagination;
mod request_context;
mod service;
mod timing;

pub use error_log::ErrorLogFilter;
pub use module::KitModule;
pub use pagination::{PageQuery, Pagination, PaginationPipe};
pub use request_context::RequestContext;
pub use service::Service;
pub use timing::TimingInterceptor;
