//! Cost of nestrs relative to plain axum, in process (no sockets), one layer at a time.
//!
//! ```text
//! cargo bench -p nestrs-bench
//! ```
//!
//! Each request goes through `tower::ServiceExt::oneshot` on the router, so the numbers
//! are the framework's own work (routing, extraction, injection, guards, interceptors)
//! with network and kernel time removed. Compare neighbouring rows: the difference is
//! what the added feature costs.

use std::time::Duration;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use nestrs::http::axum::Router;
use nestrs::http::axum::body::Body;
use nestrs::http::axum::http::{Request, StatusCode};
use nestrs_bench::overhead::{SCENARIOS, axum_layered_router, axum_router, nestrs_router};
use tower::ServiceExt;

fn request(path: &str) -> Request<Body> {
    Request::builder()
        .uri(path)
        .header("authorization", "Bearer t")
        .body(Body::empty())
        .expect("a valid request")
}

async fn call(router: &Router, path: &str) {
    let response = router.clone().oneshot(request(path)).await.expect("infallible");
    assert_eq!(response.status(), StatusCode::OK, "{path}");
}

fn overhead(c: &mut Criterion) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let nestrs = runtime.block_on(nestrs_router());
    let axum = axum_router();
    let layered = axum_layered_router();

    let mut group = c.benchmark_group("GET /hello");
    group.throughput(Throughput::Elements(1));
    group.measurement_time(Duration::from_secs(3));
    group.warm_up_time(Duration::from_secs(1));
    for (label, path) in SCENARIOS {
        let router = if path.ends_with("#layered") {
            &layered
        } else if path.starts_with("/axum") {
            &axum
        } else {
            &nestrs
        };
        let path = path.trim_end_matches("#layered");
        // Fail loudly if a variant is misconfigured, before timing anything.
        runtime.block_on(call(router, path));
        group.bench_function(*label, |b| b.to_async(&runtime).iter(|| call(router, path)));
    }
    group.finish();
}

#[allow(missing_docs)]
mod harness {
    use super::*;
    criterion_group!(benches, overhead);
}
use harness::benches;
criterion_main!(benches);
