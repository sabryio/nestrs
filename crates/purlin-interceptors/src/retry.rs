use std::time::Duration;

use purlin_http::Interceptor;
use purlin_http::axum::body::{Body, to_bytes};
use purlin_http::axum::extract::Request;
use purlin_http::axum::http::{Method, StatusCode, header};
use purlin_http::axum::middleware::Next;
use purlin_http::axum::response::Response;

use crate::support::{error_response, random_u64};

/// Runs the rest of the pipeline again when it answers with a transient failure.
///
/// Only **idempotent** methods are retried (`GET`, `HEAD`, `PUT`, `DELETE`, `OPTIONS`);
/// a `POST` is passed through untouched, because repeating it could do the work twice
/// (use [`Idempotency`](crate::Idempotency) for that). By default it retries `502`,
/// `503` and `504`, up to 3 attempts in total, waiting an exponentially growing delay
/// with jitter between them. A `Retry-After` from the response is honoured (capped at
/// the maximum delay).
///
/// The request body is buffered once (up to `max_body`, 1 MiB; larger gets `413`) so each
/// attempt receives the same body.
///
/// Place it **outside** a [`CircuitBreaker`](crate::CircuitBreaker) and
/// [`Bulkhead`](crate::Bulkhead), so each attempt is judged by them separately, and
/// **inside** [`Metrics`](crate::Metrics), so a client request counts once.
///
/// This retries the *handler pipeline*, so it helps when the failure is transient
/// (a flaky downstream the handler calls). A deterministic `503` just takes longer.
#[derive(Debug, Clone)]
pub struct Retry {
    attempts: u32,
    base: Duration,
    max_delay: Duration,
    max_body: usize,
    retry_on: fn(StatusCode) -> bool,
}

impl Retry {
    /// Up to `attempts` tries in total (the first one counts).
    ///
    /// # Panics
    /// If `attempts` is zero.
    pub fn attempts(attempts: u32) -> Self {
        assert!(attempts > 0, "Retry needs at least one attempt");
        Self {
            attempts,
            base: Duration::from_millis(100),
            max_delay: Duration::from_secs(2),
            max_body: 1024 * 1024,
            retry_on: |status| matches!(status.as_u16(), 502..=504),
        }
    }

    /// The delay before the second attempt; later ones double it.
    pub fn backoff(mut self, base: Duration) -> Self {
        self.base = base;
        self
    }

    /// The longest any single wait may be.
    pub fn max_delay(mut self, max: Duration) -> Self {
        self.max_delay = max;
        self
    }

    /// Which statuses count as transient.
    pub fn retry_on(mut self, predicate: fn(StatusCode) -> bool) -> Self {
        self.retry_on = predicate;
        self
    }

    fn delay(&self, attempt: u32, retry_after: Option<Duration>) -> Duration {
        if let Some(wait) = retry_after {
            return wait.min(self.max_delay);
        }
        let exponential = self
            .base
            .saturating_mul(2u32.saturating_pow(attempt - 1))
            .min(self.max_delay);
        // "Equal jitter": half fixed, half random, so waits spread out but never collapse to zero.
        let half = exponential / 2;
        let jitter = Duration::from_nanos(random_u64() % (half.as_nanos().max(1) as u64));
        half + jitter
    }
}

impl Default for Retry {
    fn default() -> Self {
        Self::attempts(3)
    }
}

impl Interceptor for Retry {
    async fn intercept(&self, request: Request, next: Next) -> Response {
        let idempotent = matches!(
            *request.method(),
            Method::GET | Method::HEAD | Method::PUT | Method::DELETE | Method::OPTIONS
        );
        if !idempotent || self.attempts == 1 {
            return next.run(request).await;
        }

        let (parts, body) = request.into_parts();
        let Ok(body) = to_bytes(body, self.max_body).await else {
            return error_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                "payload_too_large",
                "the request body is too large",
                None,
            );
        };

        let mut attempt = 1;
        loop {
            let mut request = Request::new(Body::from(body.clone()));
            *request.method_mut() = parts.method.clone();
            *request.uri_mut() = parts.uri.clone();
            *request.version_mut() = parts.version;
            *request.headers_mut() = parts.headers.clone();
            *request.extensions_mut() = parts.extensions.clone();

            let response = next.clone().run(request).await;
            if attempt >= self.attempts || !(self.retry_on)(response.status()) {
                return response;
            }
            let retry_after = response
                .headers()
                .get(header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok())
                .map(Duration::from_secs);
            let wait = self.delay(attempt, retry_after);
            tracing::debug!(attempt, status = response.status().as_u16(), ?wait, "retrying");
            tokio::time::sleep(wait).await;
            attempt += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    use purlin_http::axum::Router;
    use purlin_http::axum::routing::{get as get_route, post as post_route};
    use purlin_http::intercepted;

    use crate::testing::{get, post, text};

    use super::*;

    fn flaky(fail_first: u32, retry: Retry) -> (Router, Arc<AtomicU32>) {
        let calls = Arc::new(AtomicU32::new(0));
        let counter = calls.clone();
        let handler = move || {
            let n = counter.fetch_add(1, Ordering::SeqCst) + 1;
            async move {
                if n <= fail_first {
                    (StatusCode::SERVICE_UNAVAILABLE, "down".to_owned())
                } else {
                    (StatusCode::OK, format!("up after {n}"))
                }
            }
        };
        let retry = Arc::new(retry);
        let app = Router::new()
            .route("/", intercepted(get_route(handler.clone()), retry.clone()))
            .route("/write", intercepted(post_route(handler), retry));
        (app, calls)
    }

    #[tokio::test(start_paused = true)]
    async fn retries_transient_failures_until_success() {
        let (app, calls) = flaky(2, Retry::attempts(3));
        let response = get(&app, "/", &[]).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(text(response).await, "up after 3");
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn gives_up_after_the_attempt_budget() {
        let (app, calls) = flaky(10, Retry::attempts(3));
        assert_eq!(get(&app, "/", &[]).await.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn never_retries_a_post() {
        let (app, calls) = flaky(10, Retry::attempts(3));
        assert_eq!(
            post(&app, "/write", "x").await.status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn waits_between_attempts_with_exponential_backoff() {
        let (app, _) = flaky(
            2,
            Retry::attempts(3)
                .backoff(Duration::from_millis(100))
                .max_delay(Duration::from_secs(5)),
        );
        let started = tokio::time::Instant::now();
        get(&app, "/", &[]).await;
        // attempt 1 -> wait in [50,100]ms, attempt 2 -> wait in [100,200]ms
        let waited = started.elapsed();
        assert!(
            waited >= Duration::from_millis(150) && waited <= Duration::from_millis(300),
            "{waited:?}"
        );
    }

    #[test]
    fn retry_after_is_honoured_but_capped() {
        let retry = Retry::attempts(3).max_delay(Duration::from_secs(2));
        assert_eq!(retry.delay(1, Some(Duration::from_secs(1))), Duration::from_secs(1));
        assert_eq!(retry.delay(1, Some(Duration::from_secs(60))), Duration::from_secs(2));
    }
}
