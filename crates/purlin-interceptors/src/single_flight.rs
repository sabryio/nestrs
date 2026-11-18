use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use purlin_http::Interceptor;
use purlin_http::axum::extract::Request;
use purlin_http::axum::http::{HeaderValue, Method, header};
use purlin_http::axum::middleware::Next;
use purlin_http::axum::response::Response;
use tokio::sync::watch;

use crate::support::Snapshot;

type Waiters = Mutex<HashMap<String, watch::Receiver<Option<Snapshot>>>>;

/// Request coalescing: while one `GET` is running, identical concurrent `GET`s wait
/// for its response instead of running the handler again (a "thundering herd" on a
/// slow, hot endpoint becomes one query).
///
/// "Identical" means same path and query **and** same `Authorization` / `Cookie`, so
/// two callers never share a personalised response. Followers get a copy of the leader's
/// response, marked `x-coalesced: true`. When the leader's response cannot be buffered
/// (a stream, or longer than `max_body`) or the leader is cancelled, followers simply run
/// the handler themselves.
///
/// Unlike [`ResponseCache`](crate::ResponseCache) nothing is kept after the response, so
/// results are never stale.
#[derive(Debug)]
pub struct SingleFlight {
    in_flight: Waiters,
    max_body: usize,
}

impl SingleFlight {
    /// Coalesces responses up to 1 MiB.
    pub fn new() -> Self {
        Self {
            in_flight: Mutex::default(),
            max_body: 1024 * 1024,
        }
    }

    /// The largest response that is shared.
    pub fn max_body(mut self, bytes: usize) -> Self {
        self.max_body = bytes;
        self
    }
}

impl Default for SingleFlight {
    fn default() -> Self {
        Self::new()
    }
}

/// Removes the leader's entry when it finishes or is cancelled.
struct Cleanup<'a> {
    map: &'a Waiters,
    key: &'a str,
}

impl Drop for Cleanup<'_> {
    fn drop(&mut self) {
        self.map.lock().unwrap_or_else(PoisonError::into_inner).remove(self.key);
    }
}

enum Role {
    Leader(watch::Sender<Option<Snapshot>>),
    Follower(watch::Receiver<Option<Snapshot>>),
}

impl Interceptor for SingleFlight {
    async fn intercept(&self, request: Request, next: Next) -> Response {
        if request.method() != Method::GET {
            return next.run(request).await;
        }
        let key = {
            let header_of = |name| request.headers().get(name).and_then(|v| v.to_str().ok()).unwrap_or("");
            format!(
                "{}|{}|{}",
                request.uri().path_and_query().map_or("", |pq| pq.as_str()),
                header_of(header::AUTHORIZATION),
                header_of(header::COOKIE)
            )
        };

        let role = {
            let mut map = self.in_flight.lock().unwrap_or_else(PoisonError::into_inner);
            match map.get(&key) {
                Some(receiver) => Role::Follower(receiver.clone()),
                None => {
                    let (sender, receiver) = watch::channel(None);
                    map.insert(key.clone(), receiver);
                    Role::Leader(sender)
                }
            }
        };

        match role {
            Role::Follower(mut receiver) => {
                let shared = receiver
                    .wait_for(Option::is_some)
                    .await
                    .ok()
                    .and_then(|value| value.clone());
                match shared {
                    Some(snapshot) => {
                        let mut response = snapshot.to_response();
                        response
                            .headers_mut()
                            .insert("x-coalesced", HeaderValue::from_static("true"));
                        response
                    }
                    None => next.run(request).await,
                }
            }
            Role::Leader(sender) => {
                let _cleanup = Cleanup {
                    map: &self.in_flight,
                    key: &key,
                };
                let response = next.run(request).await;
                let (response, snapshot) = Snapshot::capture(response, self.max_body).await;
                if let Some(snapshot) = snapshot {
                    let _ = sender.send(Some(snapshot));
                }
                response
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    use crate::testing::{get, router_with_handler, text};

    use super::*;

    fn slow_app() -> (purlin_http::axum::Router, Arc<AtomicU32>) {
        let calls = Arc::new(AtomicU32::new(0));
        let counter = calls.clone();
        let app = router_with_handler(SingleFlight::new(), move || {
            let n = counter.fetch_add(1, Ordering::SeqCst) + 1;
            async move {
                tokio::time::sleep(Duration::from_secs(1)).await;
                format!("run {n}")
            }
        });
        (app, calls)
    }

    #[tokio::test(start_paused = true)]
    async fn concurrent_identical_gets_run_the_handler_once() {
        let (app, calls) = slow_app();
        let responses = tokio::join!(get(&app, "/", &[]), get(&app, "/", &[]), get(&app, "/", &[]));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let coalesced = [&responses.0, &responses.1, &responses.2]
            .iter()
            .filter(|r| r.headers().contains_key("x-coalesced"))
            .count();
        assert_eq!(coalesced, 2);
        assert_eq!(text(responses.0).await, "run 1");
        assert_eq!(text(responses.2).await, "run 1");
    }

    #[tokio::test(start_paused = true)]
    async fn different_callers_and_later_requests_are_not_coalesced() {
        let (app, calls) = slow_app();
        tokio::join!(
            get(&app, "/", &[("authorization", "a")]),
            get(&app, "/", &[("authorization", "b")]),
            get(&app, "/?x=1", &[]),
        );
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        get(&app, "/", &[]).await;
        assert_eq!(calls.load(Ordering::SeqCst), 4, "nothing is kept after the flight ends");
    }
}
