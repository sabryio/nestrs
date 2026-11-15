//! Entry point: a thin wrapper over `nestrs-kit`'s [`Service`].
//!
//! `blog-api --print-openapi` prints the OpenAPI document and exits; the React example
//! generates its contract from it.

use std::sync::Arc;
use std::time::Duration;

use blog_api::AppModule;
use blog_api::infrastructure::config::AppConfig;

use nestrs::http::RateLimitStore;
use nestrs::prelude::*;
use nestrs_interceptors::{Metrics, PrometheusRegistry, RequestId, SlowRequestLog};
use nestrs_kit::Service;

#[tokio::main]
async fn main() -> Result<(), AppError> {
    Service::new("Blog API", env!("CARGO_PKG_VERSION"))
        .description("Users and posts.")
        .prefix("/api")
        .rate_limit_with(|container| {
            Ok(RateLimit::per_second(50)
                .scope("global")
                .store(container.resolve::<dyn RateLimitStore>()?))
        })
        .timeout(Duration::from_secs(30))
        // Last added is outermost: the id exists before anything else runs, and metrics
        // see the final status of every request, `429`s and `504`s included.
        .interceptor(SlowRequestLog::new(Duration::from_millis(500)))
        .interceptor(Metrics::prometheus(Arc::new(PrometheusRegistry::new())).expose_at("/api/metrics"))
        .interceptor(RequestId::default())
        .run(AppModule, |container| {
            Ok(container.resolve::<AppConfig>()?.bind_addr.clone())
        })
        .await
}
