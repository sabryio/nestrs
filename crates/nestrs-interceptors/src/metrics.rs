use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use nestrs_http::Interceptor;
use nestrs_http::axum::extract::{MatchedPath, Request};
use nestrs_http::axum::http::{HeaderValue, Method, StatusCode, header};
use nestrs_http::axum::middleware::Next;
use nestrs_http::axum::response::Response;

/// One finished request, as reported to a [`MetricsSink`].
#[derive(Debug, Clone, Copy)]
pub struct RequestMetrics<'a> {
    /// HTTP method.
    pub method: &'a str,
    /// The route *template* (`/posts/{id}`), never the raw path, so label cardinality
    /// stays bounded. `"unmatched"` when no route matched.
    pub route: &'a str,
    /// Response status.
    pub status: u16,
    /// Time to produce the response head.
    pub elapsed: Duration,
}

/// Where request measurements go. Implement it for StatsD, OpenTelemetry or anything
/// else; [`PrometheusRegistry`] is the built-in one. Methods are synchronous and must
/// not block: they run on the request path.
pub trait MetricsSink: Send + Sync + 'static {
    /// A request entered the service.
    fn request_started(&self) {}
    /// A request left the service, whether it finished or was cancelled.
    fn request_ended(&self) {}
    /// A request produced a response.
    fn request_finished(&self, metrics: &RequestMetrics<'_>);
}

const BUCKETS: [f64; 11] = [0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0];

#[derive(Debug, Default)]
struct Histogram {
    buckets: [u64; BUCKETS.len()],
    sum: f64,
    count: u64,
}

#[derive(Debug, Default)]
struct Series {
    requests: BTreeMap<(String, String, u16), u64>,
    durations: BTreeMap<(String, String), Histogram>,
}

/// An in-process [`MetricsSink`] that renders the Prometheus text format.
///
/// Exposes `http_requests_total{method,route,status}`, the histogram
/// `http_request_duration_seconds{method,route}` and the gauge `http_requests_in_flight`.
#[derive(Debug, Default)]
pub struct PrometheusRegistry {
    series: Mutex<Series>,
    in_flight: AtomicI64,
}

fn escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n")
}

impl PrometheusRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Requests currently being handled.
    pub fn in_flight(&self) -> i64 {
        self.in_flight.load(Ordering::Relaxed)
    }

    /// The metrics in Prometheus text exposition format (version 0.0.4).
    pub fn render(&self) -> String {
        let series = self.series.lock().unwrap_or_else(PoisonError::into_inner);
        let mut out = String::new();
        out.push_str("# HELP http_requests_total Requests handled.\n# TYPE http_requests_total counter\n");
        for ((method, route, status), count) in &series.requests {
            let _ = writeln!(
                out,
                "http_requests_total{{method=\"{}\",route=\"{}\",status=\"{status}\"}} {count}",
                escape(method),
                escape(route)
            );
        }
        out.push_str("# HELP http_request_duration_seconds Time to produce the response.\n# TYPE http_request_duration_seconds histogram\n");
        for ((method, route), histogram) in &series.durations {
            let labels = format!("method=\"{}\",route=\"{}\"", escape(method), escape(route));
            let mut cumulative = 0;
            for (bound, count) in BUCKETS.iter().zip(histogram.buckets) {
                cumulative += count;
                let _ = writeln!(
                    out,
                    "http_request_duration_seconds_bucket{{{labels},le=\"{bound}\"}} {cumulative}"
                );
            }
            let _ = writeln!(
                out,
                "http_request_duration_seconds_bucket{{{labels},le=\"+Inf\"}} {}",
                histogram.count
            );
            let _ = writeln!(out, "http_request_duration_seconds_sum{{{labels}}} {}", histogram.sum);
            let _ = writeln!(
                out,
                "http_request_duration_seconds_count{{{labels}}} {}",
                histogram.count
            );
        }
        out.push_str(
            "# HELP http_requests_in_flight Requests being handled now.\n# TYPE http_requests_in_flight gauge\n",
        );
        let _ = writeln!(out, "http_requests_in_flight {}", self.in_flight());
        out
    }
}

impl MetricsSink for PrometheusRegistry {
    fn request_started(&self) {
        self.in_flight.fetch_add(1, Ordering::Relaxed);
    }

    fn request_ended(&self) {
        self.in_flight.fetch_sub(1, Ordering::Relaxed);
    }

    fn request_finished(&self, metrics: &RequestMetrics<'_>) {
        let mut series = self.series.lock().unwrap_or_else(PoisonError::into_inner);
        let method = metrics.method.to_owned();
        let route = metrics.route.to_owned();
        *series
            .requests
            .entry((method.clone(), route.clone(), metrics.status))
            .or_default() += 1;
        let histogram = series.durations.entry((method, route)).or_default();
        let seconds = metrics.elapsed.as_secs_f64();
        if let Some(slot) = BUCKETS.iter().position(|bound| seconds <= *bound) {
            histogram.buckets[slot] += 1;
        }
        histogram.sum += seconds;
        histogram.count += 1;
    }
}

type Render = Arc<dyn Fn() -> String + Send + Sync>;

/// Measures every request: count, latency and concurrency, labelled by method, route
/// template and status.
///
/// ```ignore
/// let registry = Arc::new(PrometheusRegistry::new());
/// app.interceptor(Metrics::prometheus(registry).expose_at("/metrics"))
/// ```
///
/// Install it near the outside so it sees the final status (including `429`s and `504`s
/// produced by interceptors inside it). The route label is the matched route template,
/// so `/posts/1` and `/posts/2` are one series, and requests no route matched share the
/// label `unmatched`. That holds both on a controller and with `App::interceptor`.
#[derive(Clone)]
pub struct Metrics {
    sink: Arc<dyn MetricsSink>,
    endpoint: Option<(String, Render)>,
}

impl Metrics {
    /// Reports to any sink.
    pub fn new(sink: Arc<dyn MetricsSink>) -> Self {
        Self { sink, endpoint: None }
    }

    /// Reports to a [`PrometheusRegistry`] and can serve it with [`expose_at`](Self::expose_at).
    pub fn prometheus(registry: Arc<PrometheusRegistry>) -> Self {
        let renderer = registry.clone();
        Self {
            sink: registry,
            endpoint: Some((String::new(), Arc::new(move || renderer.render()))),
        }
    }

    /// Answers `GET <path>` itself with the Prometheus text (only after
    /// [`prometheus`](Self::prometheus)). The path includes any global prefix.
    pub fn expose_at(mut self, path: &str) -> Self {
        if let Some((endpoint, _)) = &mut self.endpoint {
            *endpoint = path.to_owned();
        }
        self
    }
}

/// Decrements the in-flight gauge even when the request future is dropped.
struct InFlight(Arc<dyn MetricsSink>);

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.request_ended();
    }
}

impl Interceptor for Metrics {
    async fn intercept(&self, request: Request, next: Next) -> Response {
        if let Some((path, render)) = &self.endpoint
            && !path.is_empty()
            && request.method() == Method::GET
            && request.uri().path() == path
        {
            let mut response = Response::new(render().into());
            *response.status_mut() = StatusCode::OK;
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/plain; version=0.0.4; charset=utf-8"),
            );
            return response;
        }

        let method = request.method().to_string();
        let route = request
            .extensions()
            .get::<MatchedPath>()
            .map_or_else(|| "unmatched".to_owned(), |matched| matched.as_str().to_owned());
        self.sink.request_started();
        let _in_flight = InFlight(self.sink.clone());
        let started = Instant::now();
        let response = next.run(request).await;
        self.sink.request_finished(&RequestMetrics {
            method: &method,
            route: &route,
            status: response.status().as_u16(),
            elapsed: started.elapsed(),
        });
        response
    }
}

#[cfg(test)]
mod tests {
    use nestrs_http::axum::Router;
    use nestrs_http::axum::routing::get as get_route;
    use nestrs_http::intercepted;

    use crate::testing::{get, text};

    use super::*;

    #[tokio::test]
    async fn counts_requests_per_route_template_and_status() {
        let registry = Arc::new(PrometheusRegistry::new());
        let metrics = Arc::new(Metrics::prometheus(registry.clone()));
        let app = Router::new()
            .route(
                "/items/{id}",
                intercepted(get_route(|| async { "ok" }), metrics.clone()),
            )
            .route(
                "/boom",
                intercepted(get_route(|| async { StatusCode::INTERNAL_SERVER_ERROR }), metrics),
            );
        get(&app, "/items/1", &[]).await;
        get(&app, "/items/2", &[]).await;
        get(&app, "/boom", &[]).await;

        let text = registry.render();
        assert!(
            text.contains("http_requests_total{method=\"GET\",route=\"/items/{id}\",status=\"200\"} 2"),
            "{text}"
        );
        assert!(text.contains("http_requests_total{method=\"GET\",route=\"/boom\",status=\"500\"} 1"));
        assert!(text.contains("http_request_duration_seconds_count{method=\"GET\",route=\"/items/{id}\"} 2"));
        assert!(text.contains("http_request_duration_seconds_bucket{method=\"GET\",route=\"/boom\",le=\"+Inf\"} 1"));
        assert!(text.contains("http_requests_in_flight 0"));
    }

    #[tokio::test]
    async fn serves_its_own_endpoint_and_tracks_concurrency() {
        let registry = Arc::new(PrometheusRegistry::new());
        let metrics = Metrics::prometheus(registry.clone()).expose_at("/metrics");
        let app = crate::testing::router_with_handler(metrics, || async {
            tokio::time::sleep(Duration::from_millis(30)).await;
            "ok"
        });
        let slow = tokio::spawn({
            let app = app.clone();
            async move { get(&app, "/", &[]).await }
        });
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(registry.in_flight(), 1);
        slow.await.unwrap();
        assert_eq!(registry.in_flight(), 0);

        // The endpoint is answered before routing would, so mount the interceptor app-wide to reach it.
        let wrapped = Router::new().fallback(get_route(|| async { "fallback" })).layer(
            nestrs_http::axum::middleware::from_fn_with_state(
                Arc::new(Metrics::prometheus(registry.clone()).expose_at("/metrics")),
                |nestrs_http::axum::extract::State(m): nestrs_http::axum::extract::State<Arc<Metrics>>,
                 request: Request,
                 next: Next| async move { m.intercept(request, next).await },
            ),
        );
        let body = text(get(&wrapped, "/metrics", &[]).await).await;
        assert!(body.contains("# TYPE http_requests_total counter"));
    }

    #[tokio::test]
    async fn an_app_wide_layer_still_sees_the_route_template() {
        let registry = Arc::new(PrometheusRegistry::new());
        let metrics = Arc::new(Metrics::prometheus(registry.clone()));
        let app = Router::new().route("/items/{id}", get_route(|| async { "ok" })).layer(
            nestrs_http::axum::middleware::from_fn_with_state(
                metrics,
                |nestrs_http::axum::extract::State(m): nestrs_http::axum::extract::State<Arc<Metrics>>,
                 request: Request,
                 next: Next| async move { m.intercept(request, next).await },
            ),
        );
        get(&app, "/items/7", &[]).await;
        get(&app, "/nope", &[]).await;
        let text = registry.render();
        assert!(text.contains("route=\"/items/{id}\",status=\"200\""), "{text}");
        assert!(text.contains("route=\"unmatched\",status=\"404\""), "{text}");
    }

    #[test]
    fn label_values_are_escaped() {
        let registry = PrometheusRegistry::new();
        registry.request_finished(&RequestMetrics {
            method: "GET",
            route: "/a\"b\\c",
            status: 200,
            elapsed: Duration::from_millis(1),
        });
        assert!(registry.render().contains("route=\"/a\\\"b\\\\c\""));
    }
}
