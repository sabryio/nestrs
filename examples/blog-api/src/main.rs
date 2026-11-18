//! Entry point: a thin wrapper over `purlin-kit`'s [`Service`].
//!
//! `blog-api --print-openapi` prints the OpenAPI document and exits; the React example
//! generates its contract from it.

use std::sync::Arc;
use std::time::Duration;

use blog_api::AppModule;
use blog_api::infrastructure::config::AppConfig;

use purlin::http::RateLimitStore;
use purlin::prelude::*;
use purlin_interceptors::{Metrics, PrometheusRegistry, RequestId, SlowRequestLog};
use purlin_kit::Service;
use purlin_otel::{HttpTracing, OtelConfig, Telemetry};
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;

#[tokio::main]
async fn main() -> Result<(), AppError> {
    // Traces are exported when `BLOG_OTLP_ENDPOINT` names a collector's OTLP/HTTP traces URL
    // (`http://localhost:4318/v1/traces`); without it the service logs exactly as before.
    let telemetry = start_telemetry();
    let result = serve().await;
    if let Some(telemetry) = telemetry {
        telemetry.shutdown();
    }
    result
}

fn start_telemetry() -> Option<Telemetry> {
    let endpoint = std::env::var("BLOG_OTLP_ENDPOINT")
        .ok()
        .filter(|value| !value.is_empty())?;
    let config = OtelConfig::new("blog-api")
        .with_version(env!("CARGO_PKG_VERSION"))
        .with_endpoint(endpoint);
    let telemetry = match Telemetry::init(&config) {
        Ok(telemetry) => telemetry,
        Err(error) => {
            eprintln!("telemetry disabled: {error}");
            return None;
        }
    };
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,tower_http=debug"));
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .with(telemetry.layer())
        .init();
    Some(telemetry)
}

async fn serve() -> Result<(), AppError> {
    Service::new("Blog API", env!("CARGO_PKG_VERSION"))
        .description("Users and posts.")
        .prefix("/api")
        .rate_limit_with(|container| {
            Ok(RateLimit::per_second(50)
                .scope("global")
                .store(container.resolve::<dyn RateLimitStore>()?))
        })
        .without_logging_if(std::env::var_os("BLOG_OTLP_ENDPOINT").is_some_and(|v| !v.is_empty()))
        .timeout(Duration::from_secs(30))
        // Last added is outermost: the id exists before anything else runs, and metrics
        // see the final status of every request, `429`s and `504`s included.
        .interceptor(SlowRequestLog::new(Duration::from_millis(500)))
        .interceptor(Metrics::prometheus(Arc::new(PrometheusRegistry::new())).expose_at("/api/metrics"))
        .interceptor(HttpTracing::new())
        .interceptor(RequestId::default())
        .run(AppModule, |container| {
            Ok(container.resolve::<AppConfig>()?.bind_addr.clone())
        })
        .await
}
