//! # purlin-interceptors
//!
//! A catalogue of production interceptors. Each is a plain [`Interceptor`](purlin_http::Interceptor)
//! value: attach it to a route or controller with `#[interceptor(..)]`, or to the whole
//! app with `App::interceptor(..)`. Anything with state or an external system sits behind
//! a small port, so the in-memory default can be swapped (Redis, a metrics backend)
//! without touching the interceptor.
//!
//! | Group | Interceptors | Ports |
//! |---|---|---|
//! | Resilience | [`CircuitBreaker`], [`Bulkhead`], [`Retry`] | |
//! | Caching | [`ResponseCache`], [`ETag`], [`SingleFlight`] | [`CacheStore`] |
//! | Safe writes | [`Idempotency`] | [`IdempotencyStore`] |
//! | Observability | [`RequestId`], [`Metrics`], [`TraceContext`], [`Audit`], [`SlowRequestLog`] | [`MetricsSink`], [`AuditSink`] |
//! | HTTP hygiene | [`SecurityHeaders`], [`BodyLimit`], [`Deprecated`] | |
//! | Request context | [`TenantResolver`], [`LocaleNegotiator`], [`ApiVersions`] | |
//!
//! ## Order matters
//!
//! The first interceptor declared is the outermost. A good order for an app is
//! `RequestId`, `Metrics`, `SecurityHeaders`, `Timeout`, `Retry`, `CircuitBreaker`,
//! `Bulkhead`, then caching: retries are counted by metrics once per client request,
//! the breaker sees every attempt, and cache hits never occupy a bulkhead slot.
//!
//! Out of scope on purpose: **CORS** and **compression** are already `tower-http` layers
//! (`app.layer(CorsLayer..)`, `CompressionLayer`), and a **transaction per request**
//! couples HTTP to persistence, so it belongs behind your own unit-of-work port.

#![forbid(unsafe_code)]

/// Implements `FromRequestParts` (rejecting with a 500: the interceptor is not
/// installed, a wiring bug) and `OptionalFromRequestParts` for a type an interceptor
/// stores in the request extensions.
macro_rules! extension_extractor {
    ($ty:ty, $installer:literal) => {
        impl<S: Send + Sync> purlin_http::axum::extract::FromRequestParts<S> for $ty {
            type Rejection = purlin_http::HttpError;

            async fn from_request_parts(parts: &mut purlin_http::Parts, _: &S) -> Result<Self, Self::Rejection> {
                parts.extensions.get::<Self>().cloned().ok_or_else(|| {
                    purlin_http::HttpError::internal(concat!(
                        stringify!($ty),
                        " is missing: install the ",
                        $installer,
                        " interceptor"
                    ))
                })
            }
        }

        impl<S: Send + Sync> purlin_http::axum::extract::OptionalFromRequestParts<S> for $ty {
            type Rejection = purlin_http::HttpError;

            async fn from_request_parts(
                parts: &mut purlin_http::Parts,
                _: &S,
            ) -> Result<Option<Self>, Self::Rejection> {
                Ok(parts.extensions.get::<Self>().cloned())
            }
        }
    };
}

mod api_version;
mod audit;
mod body_limit;
mod bulkhead;
mod cache;
mod circuit_breaker;
mod deprecation;
mod etag;
mod idempotency;
mod locale;
mod metrics;
mod request_id;
mod retry;
mod security_headers;
mod single_flight;
mod slow_log;
mod support;
mod tenant;
mod trace_context;

#[cfg(test)]
mod testing;

pub use api_version::{ApiVersion, ApiVersions};
pub use audit::{Audit, AuditRecord, AuditSink, MemoryAuditSink, TracingAuditSink};
pub use body_limit::BodyLimit;
pub use bulkhead::Bulkhead;
pub use cache::{CacheStore, MemoryCacheStore, ResponseCache};
pub use circuit_breaker::{CircuitBreaker, CircuitState};
pub use deprecation::Deprecated;
pub use etag::ETag;
pub use idempotency::{Begin, Idempotency, IdempotencyStore, IdempotencyStorePort, MemoryIdempotencyStore};
pub use locale::{Locale, LocaleNegotiator};
pub use metrics::{Metrics, MetricsSink, PrometheusRegistry, RequestMetrics};
pub use request_id::{RequestId, RequestIdValue};
pub use retry::Retry;
pub use security_headers::SecurityHeaders;
pub use single_flight::SingleFlight;
pub use slow_log::{SlowRequest, SlowRequestLog};
pub use support::Snapshot;
pub use tenant::{Tenant, TenantResolver};
pub use trace_context::{TraceContext, TraceContextInterceptor};
