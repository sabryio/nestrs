use std::time::Instant;

use nestrs::http::axum::extract::Request;
use nestrs::http::axum::http::HeaderValue;
use nestrs::http::axum::middleware::Next;
use nestrs::http::axum::response::Response;
use nestrs::prelude::*;

/// Interceptor: reports how long the handler took in an `x-response-time-ms` header.
///
/// Attach with `#[interceptor(TimingInterceptor)]` on a controller or a route; the
/// type is provided and exported by [`KitModule`](crate::KitModule).
#[injectable]
pub struct TimingInterceptor;

impl Interceptor for TimingInterceptor {
    async fn intercept(&self, request: Request, next: Next) -> Response {
        let started = Instant::now();
        let mut response = next.run(request).await;
        let elapsed = started.elapsed().as_secs_f64() * 1000.0;
        if let Ok(value) = HeaderValue::from_str(&format!("{elapsed:.2}")) {
            response.headers_mut().insert("x-response-time-ms", value);
        }
        response
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use nestrs::http::axum::Router;
    use nestrs::http::axum::body::Body;
    use nestrs::http::axum::http::Request as HttpRequest;
    use nestrs::http::axum::routing::get;
    use nestrs::http::intercepted;
    use tower::ServiceExt;

    use super::*;

    #[tokio::test]
    async fn adds_a_millisecond_timing_header() {
        let router = Router::new().route("/", intercepted(get(|| async { "ok" }), Arc::new(TimingInterceptor)));
        let response = router
            .oneshot(HttpRequest::get("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let millis: f64 = response.headers()["x-response-time-ms"]
            .to_str()
            .unwrap()
            .parse()
            .unwrap();
        assert!(millis >= 0.0);
    }
}
