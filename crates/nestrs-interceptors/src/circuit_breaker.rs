use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use nestrs_http::Interceptor;
use nestrs_http::axum::extract::Request;
use nestrs_http::axum::http::StatusCode;
use nestrs_http::axum::middleware::Next;
use nestrs_http::axum::response::Response;
use tokio::time::Instant;

use crate::support::{error_response, with_retry_after};

/// The state of a [`CircuitBreaker`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Requests flow; failures are being counted.
    Closed,
    /// Too many failures: requests are refused without running.
    Open,
    /// The cool-down ended: one probe request decides whether to close again.
    HalfOpen,
}

#[derive(Debug, Clone, Copy)]
enum State {
    Closed { failures: u32 },
    Open { until: Instant },
    HalfOpen,
}

/// Stops calling something that keeps failing, so it can recover and callers fail fast.
///
/// After `failure_threshold` **consecutive** `5xx` responses (a [`Timeout`](nestrs_http::Timeout)
/// inside it produces `504`, so timeouts count) the breaker *opens*: for `cooldown` every
/// request is refused with `503`, code `circuit_open` and a `Retry-After`, without
/// touching the handler. Then it goes *half-open* and lets exactly one probe through:
/// success closes it, failure re-opens it for another cool-down.
///
/// One breaker guards what it is attached to. Attach a separate one to each route or
/// controller that depends on a different downstream, or the healthy ones will be cut off
/// by the unhealthy. Read [`state`](Self::state) from a `HealthIndicator` to surface it.
#[derive(Debug)]
pub struct CircuitBreaker {
    failure_threshold: u32,
    cooldown: Duration,
    state: Mutex<State>,
}

enum Admission {
    Pass,
    Probe,
    Refuse(Duration),
}

impl CircuitBreaker {
    /// Opens after `failure_threshold` consecutive failures and stays open for `cooldown`.
    ///
    /// # Panics
    /// If the threshold is zero or the cool-down is zero.
    pub fn new(failure_threshold: u32, cooldown: Duration) -> Self {
        assert!(
            failure_threshold > 0,
            "a circuit breaker needs a threshold of at least 1"
        );
        assert!(
            !cooldown.is_zero(),
            "a circuit breaker needs a cool-down greater than zero"
        );
        Self {
            failure_threshold,
            cooldown,
            state: Mutex::new(State::Closed { failures: 0 }),
        }
    }

    /// The current state.
    pub fn state(&self) -> CircuitState {
        match *self.state.lock().unwrap_or_else(PoisonError::into_inner) {
            State::Closed { .. } => CircuitState::Closed,
            State::Open { until } if Instant::now() >= until => CircuitState::HalfOpen,
            State::Open { .. } => CircuitState::Open,
            State::HalfOpen => CircuitState::HalfOpen,
        }
    }

    fn admit(&self) -> Admission {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        match *state {
            State::Closed { .. } => Admission::Pass,
            State::Open { until } => {
                let now = Instant::now();
                if now >= until {
                    *state = State::HalfOpen;
                    Admission::Probe
                } else {
                    Admission::Refuse(until - now)
                }
            }
            State::HalfOpen => Admission::Refuse(self.cooldown),
        }
    }

    fn record(&self, probe: bool, failed: bool) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let open = State::Open {
            until: Instant::now() + self.cooldown,
        };
        match (&mut *state, probe, failed) {
            (State::HalfOpen, true, true) => *state = open,
            (State::HalfOpen, true, false) => *state = State::Closed { failures: 0 },
            (State::Closed { failures }, false, true) => {
                *failures += 1;
                if *failures >= self.failure_threshold {
                    tracing::warn!(failures = *failures, "circuit opened");
                    *state = open;
                }
            }
            (State::Closed { failures }, false, false) => *failures = 0,
            // A request admitted earlier finishing after the state moved on: ignore it.
            _ => {}
        }
    }
}

/// If a probe is cancelled, re-open the circuit instead of leaving it half-open forever.
struct ProbeGuard<'a> {
    breaker: &'a CircuitBreaker,
    armed: bool,
}

impl Drop for ProbeGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.breaker.record(true, true);
        }
    }
}

impl Interceptor for CircuitBreaker {
    async fn intercept(&self, request: Request, next: Next) -> Response {
        let probe = match self.admit() {
            Admission::Pass => false,
            Admission::Probe => true,
            Admission::Refuse(wait) => {
                let message = "this endpoint is failing and is temporarily disabled";
                return with_retry_after(
                    error_response(StatusCode::SERVICE_UNAVAILABLE, "circuit_open", message, None),
                    wait.as_secs() + u64::from(wait.subsec_nanos() > 0),
                );
            }
        };
        let mut guard = ProbeGuard {
            breaker: self,
            armed: probe,
        };
        let response = next.run(request).await;
        guard.armed = false;
        self.record(probe, response.status().is_server_error());
        response
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use nestrs_http::axum::Router;
    use nestrs_http::axum::routing::get as get_route;
    use nestrs_http::intercepted;

    use crate::testing::get;

    use super::*;

    fn app(breaker: Arc<CircuitBreaker>, healthy: Arc<AtomicBool>) -> Router {
        let handler = get_route(move || {
            let ok = healthy.load(Ordering::SeqCst);
            async move {
                if ok {
                    StatusCode::OK
                } else {
                    StatusCode::INTERNAL_SERVER_ERROR
                }
            }
        });
        Router::new().route("/", intercepted(handler, breaker))
    }

    #[tokio::test(start_paused = true)]
    async fn opens_after_consecutive_failures_and_fails_fast() {
        let breaker = Arc::new(CircuitBreaker::new(3, Duration::from_secs(30)));
        let healthy = Arc::new(AtomicBool::new(false));
        let app = app(breaker.clone(), healthy);
        for _ in 0..3 {
            assert_eq!(get(&app, "/", &[]).await.status(), StatusCode::INTERNAL_SERVER_ERROR);
        }
        assert_eq!(breaker.state(), CircuitState::Open);
        let refused = get(&app, "/", &[]).await;
        assert_eq!(refused.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(refused.headers()["retry-after"], "30");
    }

    #[tokio::test(start_paused = true)]
    async fn a_success_resets_the_failure_count() {
        let breaker = Arc::new(CircuitBreaker::new(3, Duration::from_secs(30)));
        let healthy = Arc::new(AtomicBool::new(false));
        let app = app(breaker.clone(), healthy.clone());
        get(&app, "/", &[]).await;
        get(&app, "/", &[]).await;
        healthy.store(true, Ordering::SeqCst);
        get(&app, "/", &[]).await;
        healthy.store(false, Ordering::SeqCst);
        get(&app, "/", &[]).await;
        get(&app, "/", &[]).await;
        assert_eq!(breaker.state(), CircuitState::Closed, "failures were not consecutive");
    }

    #[tokio::test(start_paused = true)]
    async fn a_successful_probe_closes_it_and_a_failed_one_re_opens() {
        let breaker = Arc::new(CircuitBreaker::new(1, Duration::from_secs(10)));
        let healthy = Arc::new(AtomicBool::new(false));
        let app = app(breaker.clone(), healthy.clone());
        get(&app, "/", &[]).await;
        assert_eq!(breaker.state(), CircuitState::Open);

        tokio::time::advance(Duration::from_secs(11)).await;
        assert_eq!(breaker.state(), CircuitState::HalfOpen);
        assert_eq!(
            get(&app, "/", &[]).await.status(),
            StatusCode::INTERNAL_SERVER_ERROR,
            "the probe runs"
        );
        assert_eq!(breaker.state(), CircuitState::Open, "a failed probe re-opens");

        tokio::time::advance(Duration::from_secs(11)).await;
        healthy.store(true, Ordering::SeqCst);
        assert_eq!(get(&app, "/", &[]).await.status(), StatusCode::OK);
        assert_eq!(breaker.state(), CircuitState::Closed);
    }
}
