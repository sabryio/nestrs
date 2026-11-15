use std::sync::Arc;
use std::time::{Duration, Instant};

use nestrs_http::Interceptor;
use nestrs_http::axum::extract::Request;
use nestrs_http::axum::middleware::Next;
use nestrs_http::axum::response::Response;

/// What [`SlowRequestLog`] reports about one slow request.
#[derive(Debug, Clone)]
pub struct SlowRequest {
    /// HTTP method.
    pub method: String,
    /// Request path (without the query string, which may hold secrets).
    pub path: String,
    /// Status of the response.
    pub status: u16,
    /// How long the handler took to produce the response head.
    pub elapsed: Duration,
}

type Reporter = Arc<dyn Fn(&SlowRequest) + Send + Sync>;

/// Warns about requests that take longer than a threshold.
///
/// By default it logs at `warn` level. Give it a reporter to send the event elsewhere
/// (a metric, an alert).
#[derive(Clone)]
pub struct SlowRequestLog {
    threshold: Duration,
    reporter: Reporter,
}

impl SlowRequestLog {
    /// Reports requests slower than `threshold`.
    pub fn new(threshold: Duration) -> Self {
        Self {
            threshold,
            reporter: Arc::new(|slow| {
                tracing::warn!(
                    method = %slow.method,
                    path = %slow.path,
                    status = slow.status,
                    elapsed_ms = slow.elapsed.as_millis() as u64,
                    "slow request"
                );
            }),
        }
    }

    /// Replaces the default `tracing` report.
    pub fn reporter(mut self, reporter: impl Fn(&SlowRequest) + Send + Sync + 'static) -> Self {
        self.reporter = Arc::new(reporter);
        self
    }
}

impl Interceptor for SlowRequestLog {
    async fn intercept(&self, request: Request, next: Next) -> Response {
        let method = request.method().to_string();
        let path = request.uri().path().to_owned();
        let started = Instant::now();
        let response = next.run(request).await;
        let elapsed = started.elapsed();
        if elapsed > self.threshold {
            (self.reporter)(&SlowRequest {
                method,
                path,
                status: response.status().as_u16(),
                elapsed,
            });
        }
        response
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use crate::testing::{get, router_with_handler};

    use super::*;

    #[tokio::test]
    async fn reports_only_requests_over_the_threshold() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let log = SlowRequestLog::new(Duration::from_millis(20))
            .reporter(move |slow| sink.lock().unwrap().push(slow.clone()));
        let app = router_with_handler(log, || async {
            tokio::time::sleep(Duration::from_millis(40)).await;
            "slow"
        });
        get(&app, "/", &[]).await;
        {
            let seen = seen.lock().unwrap();
            assert_eq!(seen.len(), 1);
            assert_eq!(seen[0].path, "/");
            assert!(seen[0].elapsed >= Duration::from_millis(40));
        }

        let quick = router_with_handler(SlowRequestLog::new(Duration::from_secs(5)), || async { "fast" });
        get(&quick, "/", &[]).await;
        assert_eq!(
            seen.lock().unwrap().len(),
            1,
            "a fast request through another log reports nothing here"
        );
    }
}
