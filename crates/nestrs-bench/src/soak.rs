//! A service with every protective interceptor, instrumented for the soak test.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use nestrs::http::axum::Router;
use nestrs::prelude::*;
use nestrs_interceptors::{
    Bulkhead, CircuitBreaker, ETag, Idempotency, MemoryCacheStore, Metrics, PrometheusRegistry, RequestId,
    ResponseCache, SecurityHeaders,
};

/// Capacity of the `/soak/slow` bulkhead.
pub const BULKHEAD: usize = 8;
/// Entries the response cache may hold.
pub const CACHE_CAPACITY: usize = 64;

/// Counters the handlers update and the test reads. Shared through the container.
#[derive(Default)]
pub struct Probe {
    /// Requests currently inside the `/soak/slow` handler.
    pub slow_now: AtomicUsize,
    /// The most that were ever inside at once.
    pub slow_peak: AtomicUsize,
    /// Times the `/soak/cached` handler really ran.
    pub cached_runs: AtomicU64,
    /// Times the `/soak/write` handler really ran.
    pub writes_run: AtomicU64,
    /// While true, `/soak/flaky` fails with 500.
    pub flaky: AtomicBool,
}

/// The service under test.
#[injectable]
pub struct SoakController {
    probe: Arc<Probe>,
}

#[controller("/soak")]
#[interceptor(ETag::default())]
impl SoakController {
    #[get("/ok")]
    async fn ok(&self) -> &'static str {
        "ok"
    }

    #[get("/cached")]
    #[interceptor(ResponseCache::new(Arc::new(MemoryCacheStore::new(CACHE_CAPACITY)), Duration::from_millis(250)))]
    async fn cached(&self) -> String {
        let n = self.probe.cached_runs.fetch_add(1, Ordering::SeqCst);
        format!("run {n}")
    }

    #[get("/slow")]
    #[interceptor(Bulkhead::new(BULKHEAD))]
    async fn slow(&self) -> &'static str {
        let now = self.probe.slow_now.fetch_add(1, Ordering::SeqCst) + 1;
        self.probe.slow_peak.fetch_max(now, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(5)).await;
        self.probe.slow_now.fetch_sub(1, Ordering::SeqCst);
        "slow"
    }

    #[get("/flaky")]
    #[interceptor(CircuitBreaker::new(5, Duration::from_millis(150)))]
    async fn flaky(&self) -> Result<&'static str, HttpError> {
        if self.probe.flaky.load(Ordering::SeqCst) {
            Err(HttpError::internal("downstream failed"))
        } else {
            Ok("fine")
        }
    }

    #[get("/limited")]
    #[interceptor(RateLimit::per_second(200).scope("soak"))]
    async fn limited(&self) -> &'static str {
        "limited"
    }

    #[post("/write")]
    #[status(201)]
    #[interceptor(Idempotency::in_memory())]
    async fn write(&self) -> String {
        let n = self.probe.writes_run.fetch_add(1, Ordering::SeqCst) + 1;
        format!("write {n}")
    }
}

/// The soak app's module.
#[module(providers = [Provider::value(Probe::default())], controllers = [SoakController])]
pub struct SoakModule;

/// The built app, its metrics and its probe.
pub struct Soak {
    /// Serve requests with `oneshot`.
    pub router: Router,
    /// Everything the metrics interceptor saw.
    pub registry: Arc<PrometheusRegistry>,
    /// The handler-side counters.
    pub probe: Arc<Probe>,
}

/// Builds the app with request ids, security headers and metrics around every route.
pub async fn build() -> Soak {
    let registry = Arc::new(PrometheusRegistry::new());
    let app = App::create(SoakModule)
        .await
        .expect("the soak app builds")
        .interceptor(SecurityHeaders::default().without_hsts())
        .interceptor(Metrics::prometheus(registry.clone()))
        .interceptor(RequestId::default());
    let probe = app.container().resolve::<Probe>().expect("Probe is provided");
    Soak {
        router: app.into_router(),
        registry,
        probe,
    }
}
