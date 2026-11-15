use std::sync::Arc;
use std::time::Duration;

use nestrs_http::axum::extract::Request;
use nestrs_http::axum::middleware::Next;
use nestrs_http::axum::response::{IntoResponse, Response};
use nestrs_http::{HttpError, Interceptor};
use tokio::sync::Semaphore;

use crate::support::with_retry_after;

/// Caps how many requests run at once on a route, so one slow endpoint cannot use up
/// every worker and starve the rest of the service.
///
/// Beyond the cap a request is refused at once with `503` and `Retry-After: 1` (load
/// shedding), or, with [`queue_timeout`](Self::queue_timeout), waits that long for a
/// slot first. The slot is held until the response head is ready.
///
/// ```ignore
/// #[interceptor(Bulkhead::new(20).queue_timeout(Duration::from_millis(200)))]
/// ```
#[derive(Debug)]
pub struct Bulkhead {
    slots: Arc<Semaphore>,
    capacity: usize,
    queue_timeout: Duration,
}

impl Bulkhead {
    /// At most `max_concurrent` requests at once, no queueing.
    ///
    /// # Panics
    /// If `max_concurrent` is zero.
    pub fn new(max_concurrent: usize) -> Self {
        assert!(max_concurrent > 0, "a bulkhead needs room for at least one request");
        Self {
            slots: Arc::new(Semaphore::new(max_concurrent)),
            capacity: max_concurrent,
            queue_timeout: Duration::ZERO,
        }
    }

    /// Lets a request wait up to `timeout` for a free slot before it is refused.
    pub fn queue_timeout(mut self, timeout: Duration) -> Self {
        self.queue_timeout = timeout;
        self
    }

    /// Requests running right now.
    pub fn in_flight(&self) -> usize {
        self.capacity - self.slots.available_permits()
    }
}

impl Interceptor for Bulkhead {
    async fn intercept(&self, request: Request, next: Next) -> Response {
        let permit = if self.queue_timeout.is_zero() {
            self.slots.clone().try_acquire_owned().ok()
        } else {
            tokio::time::timeout(self.queue_timeout, self.slots.clone().acquire_owned())
                .await
                .ok()
                .and_then(Result::ok)
        };
        let Some(_permit) = permit else {
            return with_retry_after(
                HttpError::unavailable("too many concurrent requests; try again shortly", None).into_response(),
                1,
            );
        };
        next.run(request).await
    }
}

#[cfg(test)]
mod tests {
    use nestrs_http::axum::http::StatusCode;

    use crate::testing::{get, router_with_handler};

    use super::*;

    fn slow(bulkhead: Bulkhead) -> nestrs_http::axum::Router {
        router_with_handler(bulkhead, || async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            "ok"
        })
    }

    #[tokio::test(start_paused = true)]
    async fn sheds_load_beyond_the_cap() {
        let app = slow(Bulkhead::new(2));
        let (a, b, c) = tokio::join!(get(&app, "/", &[]), get(&app, "/", &[]), get(&app, "/", &[]));
        let mut statuses = [a.status(), b.status(), c.status()];
        statuses.sort();
        assert_eq!(
            statuses,
            [StatusCode::OK, StatusCode::OK, StatusCode::SERVICE_UNAVAILABLE]
        );
        let shed = [a, b, c]
            .into_iter()
            .find(|r| r.status() == StatusCode::SERVICE_UNAVAILABLE)
            .unwrap();
        assert_eq!(shed.headers()["retry-after"], "1");
    }

    #[tokio::test(start_paused = true)]
    async fn a_queue_timeout_lets_short_bursts_through() {
        let app = slow(Bulkhead::new(1).queue_timeout(Duration::from_secs(2)));
        let (a, b) = tokio::join!(get(&app, "/", &[]), get(&app, "/", &[]));
        assert_eq!((a.status(), b.status()), (StatusCode::OK, StatusCode::OK));

        let impatient = slow(Bulkhead::new(1).queue_timeout(Duration::from_millis(100)));
        let (a, b) = tokio::join!(get(&impatient, "/", &[]), get(&impatient, "/", &[]));
        let mut statuses = [a.status(), b.status()];
        statuses.sort();
        assert_eq!(statuses, [StatusCode::OK, StatusCode::SERVICE_UNAVAILABLE]);
    }

    #[tokio::test(start_paused = true)]
    async fn slots_are_released_afterwards() {
        let bulkhead = Arc::new(Bulkhead::new(1));
        assert_eq!(bulkhead.in_flight(), 0);
        let app = nestrs_http::axum::Router::new().route(
            "/",
            nestrs_http::intercepted(nestrs_http::axum::routing::get(|| async { "ok" }), bulkhead.clone()),
        );
        get(&app, "/", &[]).await;
        get(&app, "/", &[]).await;
        assert_eq!(bulkhead.in_flight(), 0);
    }
}
