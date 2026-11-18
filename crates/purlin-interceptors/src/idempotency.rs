use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use purlin_core::BoxFuture;
use purlin_http::Interceptor;
use purlin_http::Principal;
use purlin_http::axum::body::{Body, to_bytes};
use purlin_http::axum::extract::Request;
use purlin_http::axum::http::{HeaderName, HeaderValue, Method, StatusCode};
use purlin_http::axum::middleware::Next;
use purlin_http::axum::response::Response;
use tokio::time::Instant;

use crate::support::{Snapshot, digest_hex, error_response, with_retry_after};

/// An [`IdempotencyStore`] as the DI container holds it: provide this alias, inject this alias.
pub type IdempotencyStorePort = Arc<dyn IdempotencyStore>;

/// What a store says about a key when a request arrives.
#[derive(Debug, Clone)]
pub enum Begin {
    /// First time: the caller now owns the key and must `complete` or `abandon` it.
    Started,
    /// The same request finished before: replay this response.
    Replay(Snapshot),
    /// The same request is running right now.
    InFlight,
    /// The key was used with a *different* request.
    Mismatch,
    /// The store could not be reached. The request is refused (`503`) rather than run
    /// unprotected, because running it blind could repeat a write the store already saw.
    Unavailable,
}

/// Where idempotency keys and their responses are kept.
///
/// `begin` must be atomic: two concurrent calls with a new key must yield exactly one
/// `Started`. A store backed by Redis would use `SET key value NX PX ttl`.
pub trait IdempotencyStore: Send + Sync + 'static {
    /// Claims `key` for a request with this `fingerprint`, or says why it cannot.
    fn begin<'a>(&'a self, key: &'a str, fingerprint: &'a str) -> BoxFuture<'a, Begin>;
    /// Records the response of the request that owns `key`.
    fn complete<'a>(&'a self, key: &'a str, response: Snapshot) -> BoxFuture<'a, ()>;
    /// Releases `key` without a response (the request failed or was cancelled), so the
    /// client can retry it.
    fn abandon<'a>(&'a self, key: &'a str) -> BoxFuture<'a, ()>;
}

#[derive(Debug)]
struct Entry {
    fingerprint: String,
    response: Option<Snapshot>,
    expires: Instant,
}

/// An in-process [`IdempotencyStore`]. Entries expire after `ttl` (24 hours by default).
#[derive(Debug)]
pub struct MemoryIdempotencyStore {
    ttl: Duration,
    entries: Mutex<HashMap<String, Entry>>,
}

impl MemoryIdempotencyStore {
    /// Remembers keys for `ttl`.
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            entries: Mutex::default(),
        }
    }
}

impl Default for MemoryIdempotencyStore {
    fn default() -> Self {
        Self::new(Duration::from_secs(24 * 60 * 60))
    }
}

impl IdempotencyStore for MemoryIdempotencyStore {
    fn begin<'a>(&'a self, key: &'a str, fingerprint: &'a str) -> BoxFuture<'a, Begin> {
        Box::pin(async move {
            let now = Instant::now();
            let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
            if entries.len() > 10_000 {
                entries.retain(|_, entry| entry.expires > now);
            }
            if let Some(entry) = entries.get(key)
                && entry.expires > now
            {
                return if entry.fingerprint != fingerprint {
                    Begin::Mismatch
                } else {
                    entry.response.clone().map_or(Begin::InFlight, Begin::Replay)
                };
            }
            entries.insert(
                key.to_owned(),
                Entry {
                    fingerprint: fingerprint.to_owned(),
                    response: None,
                    expires: now + self.ttl,
                },
            );
            Begin::Started
        })
    }

    fn complete<'a>(&'a self, key: &'a str, response: Snapshot) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(entry) = entries.get_mut(key) {
                entry.response = Some(response);
                entry.expires = Instant::now() + self.ttl;
            }
        })
    }

    fn abandon<'a>(&'a self, key: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
            if entries.get(key).is_some_and(|entry| entry.response.is_none()) {
                entries.remove(key);
            }
        })
    }
}

/// Makes unsafe requests (`POST`, `PUT`, `PATCH`, `DELETE`) safe to retry, following the
/// `Idempotency-Key` header convention.
///
/// A client sends a unique key with a write. The first request runs and its response is
/// stored; a retry with the same key and the same request gets the **stored response
/// back** (with `idempotent-replayed: true`) without running the handler again, so a
/// timeout-and-retry never creates a second order or charges twice.
///
/// | Situation | Result |
/// |---|---|
/// | key unseen | handler runs, response stored |
/// | same key, same request, finished | stored response replayed |
/// | same key, same request, still running | `409`, code `idempotency_in_flight` |
/// | same key, different request | `422`, code `idempotency_key_reuse` |
/// | `5xx` or cancelled | nothing stored, the client may retry |
/// | store unreachable | `503` (fails closed: never run a write unprotected) |
/// | no header | runs normally, or `400` with [`required`](Self::required) |
///
/// Keys are scoped to the caller (the `Principal` set by a guard, else anonymous) and
/// to the method and path, so two users can use the same key without colliding. Because
/// the request fingerprint covers the body, the body is buffered (up to `max_body`,
/// 1 MiB by default; larger gets `413`). Install it inside the auth guard so the scope
/// is known.
#[derive(Clone)]
pub struct Idempotency {
    store: Arc<dyn IdempotencyStore>,
    header: HeaderName,
    required: bool,
    max_body: usize,
}

impl Idempotency {
    /// Uses `store`.
    pub fn new(store: Arc<dyn IdempotencyStore>) -> Self {
        Self {
            store,
            header: HeaderName::from_static("idempotency-key"),
            required: false,
            max_body: 1024 * 1024,
        }
    }

    /// Uses a fresh [`MemoryIdempotencyStore`] with a 24 hour memory.
    pub fn in_memory() -> Self {
        Self::new(Arc::new(MemoryIdempotencyStore::default()))
    }

    /// Rejects unsafe requests that carry no key with `400`.
    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }

    /// The largest request or response body that is buffered.
    pub fn max_body(mut self, bytes: usize) -> Self {
        self.max_body = bytes;
        self
    }
}

/// Releases the key if the request is cancelled mid-flight (client disconnect).
struct Claim {
    store: Arc<dyn IdempotencyStore>,
    key: String,
    armed: bool,
}

impl Drop for Claim {
    fn drop(&mut self) {
        if self.armed
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            let (store, key) = (self.store.clone(), std::mem::take(&mut self.key));
            runtime.spawn(async move { store.abandon(&key).await });
        }
    }
}

impl Interceptor for Idempotency {
    async fn intercept(&self, request: Request, next: Next) -> Response {
        if matches!(*request.method(), Method::GET | Method::HEAD | Method::OPTIONS) {
            return next.run(request).await;
        }
        let Some(supplied) = request
            .headers()
            .get(&self.header)
            .map(|value| value.to_str().map(str::to_owned))
        else {
            return if self.required {
                error_response(
                    StatusCode::BAD_REQUEST,
                    "idempotency_key_required",
                    "this request needs an Idempotency-Key header",
                    None,
                )
            } else {
                next.run(request).await
            };
        };
        let key = match supplied {
            Ok(key) if (1..=255).contains(&key.len()) => key,
            _ => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "bad_request",
                    "the Idempotency-Key must be 1 to 255 visible characters",
                    None,
                );
            }
        };

        let scope = request
            .extensions()
            .get::<Principal>()
            .map_or("anonymous", |principal| principal.subject())
            .to_owned();
        let storage_key = format!("{scope}|{}|{}|{key}", request.method(), request.uri().path());

        let (parts, body) = request.into_parts();
        let Ok(body) = to_bytes(body, self.max_body).await else {
            return error_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                "payload_too_large",
                "the request body is too large",
                None,
            );
        };
        let fingerprint = digest_hex(
            &[
                parts.method.as_str().as_bytes(),
                parts.uri.path_and_query().map_or("", |pq| pq.as_str()).as_bytes(),
                &body,
            ],
            16,
        );

        match self.store.begin(&storage_key, &fingerprint).await {
            Begin::Replay(snapshot) => {
                let mut response = snapshot.to_response();
                response
                    .headers_mut()
                    .insert("idempotent-replayed", HeaderValue::from_static("true"));
                response
            }
            Begin::InFlight => with_retry_after(
                error_response(
                    StatusCode::CONFLICT,
                    "idempotency_in_flight",
                    "a request with this Idempotency-Key is still being processed",
                    None,
                ),
                1,
            ),
            Begin::Unavailable => with_retry_after(
                error_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "idempotency protection is unavailable; retry shortly",
                    None,
                ),
                1,
            ),
            Begin::Mismatch => error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                "idempotency_key_reuse",
                "this Idempotency-Key was already used with a different request",
                None,
            ),
            Begin::Started => {
                let mut claim = Claim {
                    store: self.store.clone(),
                    key: storage_key,
                    armed: true,
                };
                let response = next.run(Request::from_parts(parts, Body::from(body))).await;
                if response.status().is_server_error() {
                    self.store.abandon(&claim.key).await;
                    claim.armed = false;
                    return response;
                }
                let (response, snapshot) = Snapshot::capture(response, self.max_body).await;
                match snapshot {
                    Some(snapshot) => self.store.complete(&claim.key, snapshot).await,
                    None => self.store.abandon(&claim.key).await,
                }
                claim.armed = false;
                response
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use purlin_http::axum::Router;
    use purlin_http::axum::routing::post;
    use purlin_http::intercepted;

    use crate::testing::{post_with, text};

    use super::*;

    fn app(idempotency: Idempotency, status: StatusCode) -> (Router, Arc<AtomicU32>) {
        let calls = Arc::new(AtomicU32::new(0));
        let counter = calls.clone();
        let handler = post(move |body: String| {
            let n = counter.fetch_add(1, Ordering::SeqCst) + 1;
            async move {
                tokio::time::sleep(Duration::from_millis(50)).await;
                (status, format!("{body}#{n}"))
            }
        });
        (
            Router::new().route("/", intercepted(handler, Arc::new(idempotency))),
            calls,
        )
    }

    const KEY: [(&str, &str); 1] = [("idempotency-key", "k1")];

    #[tokio::test(start_paused = true)]
    async fn a_retry_replays_the_first_response_without_running_the_handler() {
        let (app, calls) = app(Idempotency::in_memory(), StatusCode::CREATED);
        let first = post_with(&app, "/", &KEY, "order").await;
        assert_eq!(first.status(), StatusCode::CREATED);
        assert_eq!(text(first).await, "order#1");

        let retry = post_with(&app, "/", &KEY, "order").await;
        assert_eq!(retry.status(), StatusCode::CREATED);
        assert_eq!(retry.headers()["idempotent-replayed"], "true");
        assert_eq!(text(retry).await, "order#1");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn reusing_a_key_for_a_different_request_is_refused() {
        let (app, _) = app(Idempotency::in_memory(), StatusCode::OK);
        post_with(&app, "/", &KEY, "one").await;
        let clash = post_with(&app, "/", &KEY, "two").await;
        assert_eq!(clash.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test(start_paused = true)]
    async fn a_concurrent_duplicate_gets_409_while_the_first_runs() {
        let (app, calls) = app(Idempotency::in_memory(), StatusCode::OK);
        let (a, b) = tokio::join!(post_with(&app, "/", &KEY, "x"), post_with(&app, "/", &KEY, "x"));
        let mut statuses = [a.status(), b.status()];
        statuses.sort();
        assert_eq!(statuses, [StatusCode::OK, StatusCode::CONFLICT]);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn server_errors_are_not_stored_so_the_client_can_retry() {
        let (app, calls) = app(Idempotency::in_memory(), StatusCode::INTERNAL_SERVER_ERROR);
        post_with(&app, "/", &KEY, "x").await;
        post_with(&app, "/", &KEY, "x").await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn keys_are_optional_unless_required_and_validated() {
        let (lax, calls) = app(Idempotency::in_memory(), StatusCode::OK);
        post_with(&lax, "/", &[], "x").await;
        post_with(&lax, "/", &[], "x").await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        let (strict, _) = app(Idempotency::in_memory().required(), StatusCode::OK);
        assert_eq!(
            post_with(&strict, "/", &[], "x").await.status(),
            StatusCode::BAD_REQUEST
        );
        let long = "k".repeat(256);
        let bad = post_with(&strict, "/", &[("idempotency-key", &long)], "x").await;
        assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test(start_paused = true)]
    async fn keys_expire() {
        let store = Arc::new(MemoryIdempotencyStore::new(Duration::from_secs(60)));
        let (app, calls) = app(Idempotency::new(store), StatusCode::OK);
        post_with(&app, "/", &KEY, "x").await;
        tokio::time::advance(Duration::from_secs(61)).await;
        post_with(&app, "/", &KEY, "x").await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}
