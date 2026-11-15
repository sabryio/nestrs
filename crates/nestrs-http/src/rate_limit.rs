//! Rate limiting as an [`Interceptor`].
//!
//! ```ignore
//! #[interceptor(RateLimit::per_minute(60))]        // per client IP, on a controller or a route
//! #[interceptor(RateLimit::per_minute(5).scope("login"))]
//! app.interceptor(RateLimit::per_second(100).by(KeyBy::Global))   // whole app, outermost
//! ```
//!
//! ## Algorithm
//!
//! GCRA (generic cell rate algorithm), the "leaky bucket as a meter": a client may
//! burst `limit` requests at once, after which one more is allowed every
//! `window / limit`. Per client it stores a single timestamp, needs no background
//! task, refills smoothly (no thundering herd at a window boundary, unlike fixed
//! windows) and yields an exact `Retry-After`.
//!
//! ## Responses
//!
//! Every response carries `RateLimit-Limit`, `RateLimit-Remaining` and
//! `RateLimit-Reset` (seconds until the allowance is whole again). A rejected
//! request gets `429`, code `rate_limited`, a `Retry-After` header and
//! `details.retry_after_seconds` in the body.
//!
//! ## Where it runs
//!
//! As a route interceptor it runs *inside* guards (`guards -> interceptors ->
//! handler`), so requests a guard rejects are not counted. To throttle before
//! authentication (login brute force) or to include 404s, install it with
//! `App::interceptor`, which wraps everything.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use axum::extract::{ConnectInfo, Request};
use axum::http::{HeaderName, HeaderValue};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use nestrs_core::BoxFuture;
use tokio::time::Instant;

use crate::{HttpError, Interceptor, Principal};

const LIMIT: HeaderName = HeaderName::from_static("ratelimit-limit");
const REMAINING: HeaderName = HeaderName::from_static("ratelimit-remaining");
const RESET: HeaderName = HeaderName::from_static("ratelimit-reset");

/// The outcome of counting one request against a limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decision {
    /// Whether the request may proceed.
    pub allowed: bool,
    /// Requests that would still be allowed right now.
    pub remaining: u32,
    /// When rejected, how long until one request would be allowed; otherwise zero.
    pub retry_after: Duration,
    /// How long until the client's full allowance is back.
    pub reset_after: Duration,
}

/// A failure of the store itself (a lost Redis connection, say).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("rate limit store failed: {0}")]
pub struct StoreError(pub String);

/// Port: where per-client counters live.
///
/// [`MemoryStore`] keeps them in the process, which is exact for one instance. Behind
/// several instances, implement this over Redis or similar so they share counts.
///
/// Contract: `hit` counts one request for `key` **atomically** and decides it. A
/// store error makes [`RateLimit`] *fail open* (the request proceeds and the error is
/// logged): an outage of the limiter should not become an outage of the API.
pub trait RateLimitStore: Send + Sync + 'static {
    /// Counts one request for `key` against `limit` requests per `window`.
    fn hit<'a>(&'a self, key: &'a str, limit: u32, window: Duration) -> BoxFuture<'a, Result<Decision, StoreError>>;
}

/// The GCRA decision for one request. Pure: callers store the returned timestamp.
///
/// `stored` is the key's theoretical arrival time (when its allowance is whole
/// again). Returns the decision and, when allowed, the new value to store.
fn gcra(stored: Option<Instant>, now: Instant, limit: u32, window: Duration) -> (Decision, Option<Instant>) {
    // One request "costs" this much of the window; never zero, so division is safe.
    let interval = (window / limit).max(Duration::from_nanos(1));
    let tat = stored.filter(|tat| *tat > now).unwrap_or(now);
    let new_tat = tat + interval;
    let edge = now + window;

    if new_tat <= edge {
        let remaining = (edge.saturating_duration_since(new_tat).as_nanos() / interval.as_nanos()) as u32;
        let decision = Decision {
            allowed: true,
            remaining,
            retry_after: Duration::ZERO,
            reset_after: new_tat.saturating_duration_since(now),
        };
        (decision, Some(new_tat))
    } else {
        let decision = Decision {
            allowed: false,
            remaining: 0,
            retry_after: new_tat.saturating_duration_since(edge),
            reset_after: tat.saturating_duration_since(now),
        };
        (decision, None)
    }
}

/// In-process [`RateLimitStore`] with bounded memory.
///
/// Expired entries are swept regularly. If an attacker fills the table with distinct
/// keys, once `capacity` live keys exist new keys are *not tracked* (their requests
/// pass, and a warning is logged) instead of growing without bound or locking out
/// legitimate clients.
#[derive(Debug)]
pub struct MemoryStore {
    capacity: usize,
    state: Mutex<MemoryState>,
}

#[derive(Debug, Default)]
struct MemoryState {
    tats: HashMap<String, Instant>,
    hits: u64,
}

/// Sweep expired keys every this many hits.
const SWEEP_EVERY: u64 = 1024;

impl MemoryStore {
    /// A store tracking up to 100 000 clients at once.
    pub fn new() -> Self {
        Self::with_capacity(100_000)
    }

    /// A store tracking up to `capacity` clients at once.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            capacity,
            state: Mutex::default(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, MemoryState> {
        // The state is plain data; a panic elsewhere cannot leave it half-updated.
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Number of clients currently tracked (including not-yet-swept expired ones).
    pub fn tracked(&self) -> usize {
        self.lock().tats.len()
    }
}

impl Default for MemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

impl RateLimitStore for MemoryStore {
    fn hit<'a>(&'a self, key: &'a str, limit: u32, window: Duration) -> BoxFuture<'a, Result<Decision, StoreError>> {
        Box::pin(async move {
            let now = Instant::now();
            let mut state = self.lock();
            state.hits += 1;
            if state.hits % SWEEP_EVERY == 0 {
                state.tats.retain(|_, tat| *tat > now);
            }

            let (decision, new_tat) = gcra(state.tats.get(key).copied(), now, limit, window);
            if let Some(tat) = new_tat {
                if !state.tats.contains_key(key) && state.tats.len() >= self.capacity {
                    state.tats.retain(|_, tat| *tat > now);
                    if state.tats.len() >= self.capacity {
                        tracing::warn!(
                            capacity = self.capacity,
                            "rate limit table is full; not tracking a new client"
                        );
                        return Ok(decision);
                    }
                }
                state.tats.insert(key.to_owned(), tat);
            }
            Ok(decision)
        })
    }
}

/// Derives a client key from a request; see [`KeyBy::Custom`].
pub type KeyFn = Arc<dyn Fn(&Request) -> Option<String> + Send + Sync>;

/// Who a request is counted against.
#[derive(Clone)]
pub enum KeyBy {
    /// The client's IP address (IPv6 clients are grouped by their /64). The default.
    Ip,
    /// One shared allowance for everyone, e.g. to protect an expensive endpoint.
    Global,
    /// The value of a request header, such as an API key. Requests without the header
    /// fall back to their IP, so omitting it never escapes the limit. Put this
    /// behind authentication: an unauthenticated caller can invent header values.
    Header(&'static str),
    /// The authenticated caller's [`Principal`] subject; requests without one fall back
    /// to their IP. Only meaningful behind a guard that authenticates (route-level
    /// interceptors run after guards).
    Caller,
    /// Whatever the function returns (a user id from a guard's request extension,
    /// say); `None` falls back to the IP.
    Custom(KeyFn),
}

impl std::fmt::Debug for KeyBy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ip => f.write_str("Ip"),
            Self::Global => f.write_str("Global"),
            Self::Caller => f.write_str("Caller"),
            Self::Header(name) => f.debug_tuple("Header").field(name).finish(),
            Self::Custom(_) => f.write_str("Custom(..)"),
        }
    }
}

/// Process-wide counter naming limiters that were not given a [`scope`](RateLimit::scope).
static NEXT_SCOPE: AtomicU64 = AtomicU64::new(1);

/// Limits how often a client may call a route: see the [module docs](self).
#[derive(Clone)]
pub struct RateLimit {
    limit: u32,
    window: Duration,
    key: KeyBy,
    scope: Arc<str>,
    store: Arc<dyn RateLimitStore>,
    trust_proxy: bool,
}

impl RateLimit {
    /// Allows `limit` requests per `window` per client, with its own in-memory store.
    ///
    /// # Panics
    /// If `limit` or `window` is zero: that limits nothing sensibly and is a typo.
    pub fn new(limit: u32, window: Duration) -> Self {
        assert!(limit > 0, "RateLimit needs a limit greater than zero");
        assert!(!window.is_zero(), "RateLimit needs a window greater than zero");
        Self {
            limit,
            window,
            key: KeyBy::Ip,
            scope: format!("limiter-{}", NEXT_SCOPE.fetch_add(1, Ordering::Relaxed)).into(),
            store: Arc::new(MemoryStore::new()),
            trust_proxy: false,
        }
    }

    /// `limit` requests per second.
    pub fn per_second(limit: u32) -> Self {
        Self::new(limit, Duration::from_secs(1))
    }

    /// `limit` requests per minute.
    pub fn per_minute(limit: u32) -> Self {
        Self::new(limit, Duration::from_secs(60))
    }

    /// `limit` requests per hour.
    pub fn per_hour(limit: u32) -> Self {
        Self::new(limit, Duration::from_secs(3600))
    }

    /// Counts requests against `key` instead of the client IP.
    pub fn by(mut self, key: KeyBy) -> Self {
        self.key = key;
        self
    }

    /// Names this limit. Counters are keyed by `scope` + client, so two limits sharing
    /// a store do not interfere, and the same name on several instances of an app
    /// shares one allowance. Set it whenever the store is shared; the default is a
    /// per-process counter, which is only meaningful for the in-memory store.
    pub fn scope(mut self, name: impl Into<Arc<str>>) -> Self {
        self.scope = name.into();
        self
    }

    /// Keeps counters in `store` (for example one shared across instances).
    pub fn store(mut self, store: Arc<dyn RateLimitStore>) -> Self {
        self.store = store;
        self
    }

    /// Reads the client IP from the first `X-Forwarded-For` entry instead of the
    /// socket. Enable it **only** behind a proxy that sets or overwrites that header;
    /// otherwise any client can pick its own bucket.
    pub fn trust_proxy_headers(mut self) -> Self {
        self.trust_proxy = true;
        self
    }

    fn client(&self, request: &Request) -> String {
        match &self.key {
            KeyBy::Global => "global".to_owned(),
            KeyBy::Ip => self.ip(request),
            KeyBy::Caller => request
                .extensions()
                .get::<Principal>()
                .map(|principal| format!("caller:{}", principal.subject()))
                .unwrap_or_else(|| self.ip(request)),
            KeyBy::Header(name) => header_value(request, name)
                .map(|value| format!("header:{value}"))
                .unwrap_or_else(|| self.ip(request)),
            KeyBy::Custom(key) => key(request)
                .map(|value| format!("custom:{value}"))
                .unwrap_or_else(|| self.ip(request)),
        }
    }

    fn ip(&self, request: &Request) -> String {
        let forwarded = self
            .trust_proxy
            .then(|| header_value(request, "x-forwarded-for"))
            .flatten()
            .and_then(|list| {
                list.split(',')
                    .next()
                    .and_then(|first| first.trim().parse::<IpAddr>().ok())
            });
        let peer = || {
            request
                .extensions()
                .get::<ConnectInfo<SocketAddr>>()
                .map(|info| info.0.ip())
        };
        match forwarded.or_else(peer) {
            Some(IpAddr::V4(ip)) => ip.to_string(),
            Some(IpAddr::V6(ip)) => {
                // A single host controls a whole /64, so count the network, not the address.
                let [a, b, c, d, ..] = ip.segments();
                format!("{a:x}:{b:x}:{c:x}:{d:x}::/64")
            }
            // No socket information (e.g. in-process tests): one shared bucket.
            None => "unknown".to_owned(),
        }
    }

    fn annotate(&self, response: &mut Response, decision: &Decision) {
        let headers = response.headers_mut();
        headers.insert(LIMIT, HeaderValue::from(self.limit));
        headers.insert(REMAINING, HeaderValue::from(decision.remaining));
        headers.insert(RESET, HeaderValue::from(ceil_seconds(decision.reset_after)));
    }
}

impl std::fmt::Debug for RateLimit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RateLimit")
            .field("limit", &self.limit)
            .field("window", &self.window)
            .field("key", &self.key)
            .field("scope", &self.scope)
            .finish_non_exhaustive()
    }
}

fn header_value<'a>(request: &'a Request, name: &str) -> Option<&'a str> {
    request
        .headers()
        .get(name)?
        .to_str()
        .ok()
        .filter(|value| !value.is_empty())
}

fn ceil_seconds(duration: Duration) -> u64 {
    duration.as_secs() + u64::from(duration.subsec_nanos() > 0)
}

impl Interceptor for RateLimit {
    async fn intercept(&self, request: Request, next: Next) -> Response {
        let key = format!("{}:{}", self.scope, self.client(&request));
        let decision = match self.store.hit(&key, self.limit, self.window).await {
            Ok(decision) => decision,
            Err(error) => {
                tracing::error!(%error, "rate limit store failed; letting the request through");
                return next.run(request).await;
            }
        };

        let mut response = if decision.allowed {
            next.run(request).await
        } else {
            tracing::debug!(%key, retry_after = ?decision.retry_after, "rate limited");
            HttpError::too_many_requests(decision.retry_after).into_response()
        };
        self.annotate(&mut response, &decision);
        response
    }
}

#[cfg(test)]
mod tests {
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request as HttpRequest, StatusCode};
    use axum::routing::get;
    use serde_json::Value;
    use tower::ServiceExt;

    use super::*;
    use crate::intercepted;

    fn router(limit: RateLimit) -> Router {
        Router::new().route("/", intercepted(get(|| async { "ok" }), Arc::new(limit)))
    }

    fn peer(address: &str) -> HttpRequest<Body> {
        let mut request = HttpRequest::get("/").body(Body::empty()).unwrap();
        let address: SocketAddr = address.parse().unwrap();
        request.extensions_mut().insert(ConnectInfo(address));
        request
    }

    async fn call(router: &Router, request: HttpRequest<Body>) -> Response {
        router.clone().oneshot(request).await.unwrap()
    }

    fn header(response: &Response, name: &str) -> String {
        response.headers()[name].to_str().unwrap().to_owned()
    }

    #[tokio::test(start_paused = true)]
    async fn bursts_up_to_the_limit_then_refills_smoothly() {
        // 3 per 3 s: three at once, then one more every second.
        let store = MemoryStore::new();
        let hit = || store.hit("k", 3, Duration::from_secs(3));
        let remaining: Vec<_> = [hit().await.unwrap(), hit().await.unwrap(), hit().await.unwrap()]
            .iter()
            .map(|d| (d.allowed, d.remaining))
            .collect();
        assert_eq!(remaining, [(true, 2), (true, 1), (true, 0)]);

        let denied = hit().await.unwrap();
        assert!(!denied.allowed);
        assert_eq!(denied.retry_after, Duration::from_secs(1));

        tokio::time::advance(Duration::from_millis(400)).await;
        assert_eq!(
            hit().await.unwrap().retry_after,
            Duration::from_millis(600),
            "the wait shrinks as time passes"
        );

        tokio::time::advance(Duration::from_millis(600)).await;
        assert!(hit().await.unwrap().allowed, "one request is back after window/limit");
        assert!(!hit().await.unwrap().allowed, "and only one");

        tokio::time::advance(Duration::from_secs(60)).await;
        assert_eq!(
            hit().await.unwrap().remaining,
            2,
            "idle clients get the full burst back"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn clients_do_not_share_an_allowance() {
        let store = MemoryStore::new();
        assert!(store.hit("a", 1, Duration::from_secs(60)).await.unwrap().allowed);
        assert!(!store.hit("a", 1, Duration::from_secs(60)).await.unwrap().allowed);
        assert!(store.hit("b", 1, Duration::from_secs(60)).await.unwrap().allowed);
    }

    #[tokio::test(start_paused = true)]
    async fn rejections_are_a_429_envelope_with_retry_after() {
        let app = router(RateLimit::new(2, Duration::from_secs(10)));
        for expected in ["1", "0"] {
            let ok = call(&app, peer("10.0.0.1:1000")).await;
            assert_eq!(ok.status(), StatusCode::OK);
            assert_eq!(header(&ok, "ratelimit-limit"), "2");
            assert_eq!(header(&ok, "ratelimit-remaining"), expected);
        }

        let limited = call(&app, peer("10.0.0.1:1000")).await;
        assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(header(&limited, "retry-after"), "5");
        assert_eq!(header(&limited, "ratelimit-remaining"), "0");
        let body: Value = serde_json::from_slice(&to_bytes(limited.into_body(), usize::MAX).await.unwrap()).unwrap();
        assert_eq!(body["error"]["code"], "rate_limited");
        assert_eq!(body["error"]["details"]["retry_after_seconds"], 5);

        // Someone else is unaffected.
        assert_eq!(call(&app, peer("10.0.0.2:1000")).await.status(), StatusCode::OK);
    }

    #[tokio::test(start_paused = true)]
    async fn ipv6_clients_are_counted_per_64_network() {
        let app = router(RateLimit::per_minute(1));
        assert_eq!(call(&app, peer("[2001:db8:1:2::1]:1")).await.status(), StatusCode::OK);
        // Same /64, different address: the same client for limiting purposes.
        assert_eq!(
            call(&app, peer("[2001:db8:1:2:ffff::9]:1")).await.status(),
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(call(&app, peer("[2001:db8:1:3::1]:1")).await.status(), StatusCode::OK);
    }

    #[tokio::test(start_paused = true)]
    async fn forwarded_for_is_ignored_unless_the_proxy_is_trusted() {
        let forwarded = |ip: &str| {
            let mut request = peer("10.0.0.1:1");
            request.headers_mut().insert("x-forwarded-for", ip.parse().unwrap());
            request
        };

        let untrusted = router(RateLimit::per_minute(1));
        assert_eq!(call(&untrusted, forwarded("1.1.1.1")).await.status(), StatusCode::OK);
        assert_eq!(
            call(&untrusted, forwarded("2.2.2.2")).await.status(),
            StatusCode::TOO_MANY_REQUESTS,
            "a spoofed header must not pick a fresh bucket"
        );

        let trusted = router(RateLimit::per_minute(1).trust_proxy_headers());
        assert_eq!(
            call(&trusted, forwarded("1.1.1.1, 10.9.9.9")).await.status(),
            StatusCode::OK
        );
        assert_eq!(call(&trusted, forwarded("2.2.2.2")).await.status(), StatusCode::OK);
        assert_eq!(
            call(&trusted, forwarded("1.1.1.1")).await.status(),
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(
            call(&trusted, forwarded("not an ip")).await.status(),
            StatusCode::OK,
            "garbage falls back to the peer"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn header_keys_fall_back_to_the_ip_when_absent() {
        let app = router(RateLimit::per_minute(1).by(KeyBy::Header("x-api-key")));
        let keyed = |key: &str| {
            let mut request = peer("10.0.0.1:1");
            request.headers_mut().insert("x-api-key", key.parse().unwrap());
            request
        };
        assert_eq!(call(&app, keyed("alpha")).await.status(), StatusCode::OK);
        assert_eq!(
            call(&app, keyed("beta")).await.status(),
            StatusCode::OK,
            "another key, another allowance"
        );
        assert_eq!(call(&app, keyed("alpha")).await.status(), StatusCode::TOO_MANY_REQUESTS);

        assert_eq!(call(&app, peer("10.0.0.1:1")).await.status(), StatusCode::OK);
        assert_eq!(
            call(&app, peer("10.0.0.1:1")).await.status(),
            StatusCode::TOO_MANY_REQUESTS,
            "omitting the header does not escape the limit"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn callers_are_counted_by_principal_and_fall_back_to_the_ip() {
        let app = router(RateLimit::per_minute(1).by(KeyBy::Caller));
        let as_user = |name: &str| {
            let mut request = peer("10.0.0.1:1");
            request.extensions_mut().insert(Principal::new(name));
            request
        };
        assert_eq!(call(&app, as_user("ann")).await.status(), StatusCode::OK);
        assert_eq!(
            call(&app, as_user("bob")).await.status(),
            StatusCode::OK,
            "same IP, another caller"
        );
        assert_eq!(call(&app, as_user("ann")).await.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            call(&app, peer("10.0.0.1:1")).await.status(),
            StatusCode::OK,
            "anonymous: by IP"
        );
        assert_eq!(
            call(&app, peer("10.0.0.1:1")).await.status(),
            StatusCode::TOO_MANY_REQUESTS
        );
    }

    #[tokio::test(start_paused = true)]
    async fn global_and_custom_keys() {
        let global = router(RateLimit::per_minute(1).by(KeyBy::Global));
        assert_eq!(call(&global, peer("10.0.0.1:1")).await.status(), StatusCode::OK);
        assert_eq!(
            call(&global, peer("10.0.0.2:1")).await.status(),
            StatusCode::TOO_MANY_REQUESTS
        );

        let custom = router(RateLimit::per_minute(1).by(KeyBy::Custom(Arc::new(|request: &Request| {
            request.uri().query().map(str::to_owned)
        }))));
        let with = |query: &str| HttpRequest::get(format!("/?{query}")).body(Body::empty()).unwrap();
        assert_eq!(call(&custom, with("u=1")).await.status(), StatusCode::OK);
        assert_eq!(call(&custom, with("u=2")).await.status(), StatusCode::OK);
        assert_eq!(call(&custom, with("u=1")).await.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test(start_paused = true)]
    async fn scopes_separate_limits_that_share_a_store() {
        let store: Arc<dyn RateLimitStore> = Arc::new(MemoryStore::new());
        let build = |scope: &str| router(RateLimit::per_minute(1).scope(scope.to_owned()).store(store.clone()));
        let (login, search, login_again) = (build("login"), build("search"), build("login"));

        assert_eq!(call(&login, peer("10.0.0.1:1")).await.status(), StatusCode::OK);
        assert_eq!(
            call(&search, peer("10.0.0.1:1")).await.status(),
            StatusCode::OK,
            "different scope"
        );
        assert_eq!(
            call(&login_again, peer("10.0.0.1:1")).await.status(),
            StatusCode::TOO_MANY_REQUESTS,
            "same scope on another instance shares the allowance"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn the_memory_store_stays_bounded() {
        let store = MemoryStore::with_capacity(2);
        let window = Duration::from_secs(60);
        for key in ["a", "b"] {
            assert!(store.hit(key, 1, window).await.unwrap().allowed);
        }
        // Full: a third client is let through but not tracked.
        assert!(store.hit("c", 1, window).await.unwrap().allowed);
        assert!(
            store.hit("c", 1, window).await.unwrap().allowed,
            "untracked clients are not limited"
        );
        assert_eq!(store.tracked(), 2);

        // Once old entries expire, new clients are tracked again.
        tokio::time::advance(window * 2).await;
        assert!(store.hit("c", 1, window).await.unwrap().allowed);
        assert!(!store.hit("c", 1, window).await.unwrap().allowed);
        assert!(store.tracked() <= 2);
    }

    struct Broken;
    impl RateLimitStore for Broken {
        fn hit<'a>(&'a self, _: &'a str, _: u32, _: Duration) -> BoxFuture<'a, Result<Decision, StoreError>> {
            Box::pin(async { Err(StoreError("connection refused".into())) })
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_failing_store_fails_open() {
        let app = router(RateLimit::per_minute(1).store(Arc::new(Broken)));
        for _ in 0..3 {
            assert_eq!(call(&app, peer("10.0.0.1:1")).await.status(), StatusCode::OK);
        }
    }

    #[test]
    #[should_panic(expected = "limit greater than zero")]
    fn a_zero_limit_is_rejected_at_construction() {
        let _ = RateLimit::per_minute(0);
    }
}
