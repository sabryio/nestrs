use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use nestrs_core::BoxFuture;
use nestrs_http::Interceptor;
use nestrs_http::axum::extract::Request;
use nestrs_http::axum::http::{HeaderName, HeaderValue, Method, StatusCode, header};
use nestrs_http::axum::middleware::Next;
use nestrs_http::axum::response::Response;
use tokio::time::Instant;

use crate::support::Snapshot;

/// Where cached responses live. Best effort by contract: a store that fails must behave
/// as a miss (`get` returns `None`, `put` does nothing), because a cache outage should
/// slow the service down, never take it down.
pub trait CacheStore: Send + Sync + 'static {
    /// The live entry for `key`, if any.
    fn get<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Option<Snapshot>>;
    /// Stores `value` for `ttl`.
    fn put<'a>(&'a self, key: String, value: Snapshot, ttl: Duration) -> BoxFuture<'a, ()>;
}

/// A bounded in-process [`CacheStore`]. When full it drops expired entries first, then
/// the entry closest to expiry.
#[derive(Debug)]
pub struct MemoryCacheStore {
    capacity: usize,
    entries: Mutex<HashMap<String, (Snapshot, Instant)>>,
}

impl MemoryCacheStore {
    /// A store holding at most `capacity` entries.
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            entries: Mutex::default(),
        }
    }

    /// Number of stored entries, including any not yet swept after expiry.
    pub fn len(&self) -> usize {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner).len()
    }

    /// Whether the store holds nothing.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl CacheStore for MemoryCacheStore {
    fn get<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Option<Snapshot>> {
        Box::pin(async move {
            let now = Instant::now();
            let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
            match entries.get(key) {
                Some((snapshot, expires)) if *expires > now => Some(snapshot.clone()),
                Some(_) => {
                    entries.remove(key);
                    None
                }
                None => None,
            }
        })
    }

    fn put<'a>(&'a self, key: String, value: Snapshot, ttl: Duration) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let now = Instant::now();
            let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
            if entries.len() >= self.capacity && !entries.contains_key(&key) {
                entries.retain(|_, (_, expires)| *expires > now);
                if entries.len() >= self.capacity
                    && let Some(oldest) = entries
                        .iter()
                        .min_by_key(|(_, (_, expires))| *expires)
                        .map(|(k, _)| k.clone())
                {
                    entries.remove(&oldest);
                }
            }
            entries.insert(key, (value, now + ttl));
        })
    }
}

/// Caches successful `GET` responses so the handler does not run again within the TTL.
///
/// What is cached: `200` responses with a declared length up to `max_body` (1 MiB). What
/// is not: other methods, streams, responses with `Set-Cookie`, or `Cache-Control`
/// of `no-store` / `private`. The key is path, query and the values of the headers named
/// with [`vary`](Self::vary).
///
/// **Personalised responses are never shared by default.** A request carrying
/// `Authorization` or `Cookie` bypasses the cache entirely, so one user's data cannot
/// be served to another. Call [`cache_authenticated`](Self::cache_authenticated) only for
/// routes whose output does not depend on who asks.
///
/// Responses carry `x-cache: HIT` or `x-cache: MISS`.
#[derive(Clone)]
pub struct ResponseCache {
    store: Arc<dyn CacheStore>,
    ttl: Duration,
    vary: Vec<HeaderName>,
    max_body: usize,
    cache_authenticated: bool,
}

const X_CACHE: HeaderName = HeaderName::from_static("x-cache");

impl ResponseCache {
    /// Caches in `store` for `ttl`.
    pub fn new(store: Arc<dyn CacheStore>, ttl: Duration) -> Self {
        Self {
            store,
            ttl,
            vary: Vec::new(),
            max_body: 1024 * 1024,
            cache_authenticated: false,
        }
    }

    /// Caches in a fresh [`MemoryCacheStore`] of 1000 entries.
    pub fn in_memory(ttl: Duration) -> Self {
        Self::new(Arc::new(MemoryCacheStore::new(1000)), ttl)
    }

    /// Keeps separate entries per value of these request headers (`accept-language`).
    pub fn vary(mut self, headers: impl IntoIterator<Item = &'static str>) -> Self {
        self.vary.extend(headers.into_iter().map(HeaderName::from_static));
        self
    }

    /// The largest body that is cached.
    pub fn max_body(mut self, bytes: usize) -> Self {
        self.max_body = bytes;
        self
    }

    /// Also caches requests that carry `Authorization` or `Cookie`.
    pub fn cache_authenticated(mut self) -> Self {
        self.cache_authenticated = true;
        self
    }

    fn key(&self, request: &Request) -> String {
        let mut key = request
            .uri()
            .path_and_query()
            .map_or_else(|| request.uri().path().to_owned(), |pq| pq.as_str().to_owned());
        for name in &self.vary {
            let value = request.headers().get(name).and_then(|v| v.to_str().ok()).unwrap_or("");
            key.push('|');
            key.push_str(name.as_str());
            key.push('=');
            key.push_str(value);
        }
        key
    }
}

fn cacheable(response: &Response) -> bool {
    let headers = response.headers();
    let directive_blocks = headers
        .get_all(header::CACHE_CONTROL)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .any(|value| value.contains("no-store") || value.contains("private"));
    response.status() == StatusCode::OK && !headers.contains_key(header::SET_COOKIE) && !directive_blocks
}

impl Interceptor for ResponseCache {
    async fn intercept(&self, request: Request, next: Next) -> Response {
        if request.method() != Method::GET {
            return next.run(request).await;
        }
        let personal =
            request.headers().contains_key(header::AUTHORIZATION) || request.headers().contains_key(header::COOKIE);
        if personal && !self.cache_authenticated {
            return next.run(request).await;
        }

        let key = self.key(&request);
        if let Some(hit) = self.store.get(&key).await {
            let mut response = hit.to_response();
            response.headers_mut().insert(X_CACHE, HeaderValue::from_static("HIT"));
            return response;
        }

        let response = next.run(request).await;
        if !cacheable(&response) {
            return response;
        }
        let (mut response, snapshot) = Snapshot::capture(response, self.max_body).await;
        if let Some(snapshot) = snapshot {
            self.store.put(key, snapshot, self.ttl).await;
            response.headers_mut().insert(X_CACHE, HeaderValue::from_static("MISS"));
        }
        response
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use crate::testing::{get, router_with_handler, text};

    use super::*;

    fn counting_app(cache: ResponseCache) -> (nestrs_http::axum::Router, Arc<AtomicU32>) {
        let calls = Arc::new(AtomicU32::new(0));
        let counter = calls.clone();
        let app = router_with_handler(cache, move || {
            let n = counter.fetch_add(1, Ordering::SeqCst) + 1;
            async move { format!("call {n}") }
        });
        (app, calls)
    }

    #[tokio::test(start_paused = true)]
    async fn serves_hits_until_the_ttl_ends() {
        let (app, calls) = counting_app(ResponseCache::in_memory(Duration::from_secs(10)));
        let first = get(&app, "/", &[]).await;
        assert_eq!(first.headers()["x-cache"], "MISS");
        assert_eq!(text(first).await, "call 1");

        let second = get(&app, "/", &[]).await;
        assert_eq!(second.headers()["x-cache"], "HIT");
        assert_eq!(text(second).await, "call 1");
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        tokio::time::advance(Duration::from_secs(11)).await;
        assert_eq!(text(get(&app, "/", &[]).await).await, "call 2");
    }

    #[tokio::test]
    async fn personalised_requests_bypass_the_cache_unless_opted_in() {
        let (app, calls) = counting_app(ResponseCache::in_memory(Duration::from_secs(10)));
        get(&app, "/", &[("authorization", "Bearer a")]).await;
        get(&app, "/", &[("authorization", "Bearer a")]).await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        let (shared, calls) = counting_app(ResponseCache::in_memory(Duration::from_secs(10)).cache_authenticated());
        get(&shared, "/", &[("authorization", "Bearer a")]).await;
        get(&shared, "/", &[("authorization", "Bearer b")]).await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn vary_headers_and_query_strings_get_separate_entries() {
        let (app, calls) = counting_app(ResponseCache::in_memory(Duration::from_secs(10)).vary(["accept-language"]));
        get(&app, "/", &[("accept-language", "en")]).await;
        get(&app, "/", &[("accept-language", "ar")]).await;
        get(&app, "/?page=2", &[("accept-language", "en")]).await;
        get(&app, "/", &[("accept-language", "en")]).await;
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn no_store_and_set_cookie_are_not_cached() {
        let app = router_with_handler(ResponseCache::in_memory(Duration::from_secs(10)), || async {
            ([(header::CACHE_CONTROL, "no-store")], "x")
        });
        assert!(!get(&app, "/", &[]).await.headers().contains_key("x-cache"));
        let cookie = router_with_handler(ResponseCache::in_memory(Duration::from_secs(10)), || async {
            ([(header::SET_COOKIE, "a=b")], "x")
        });
        assert!(!get(&cookie, "/", &[]).await.headers().contains_key("x-cache"));
    }

    #[tokio::test(start_paused = true)]
    async fn the_memory_store_is_bounded() {
        let store = MemoryCacheStore::new(2);
        let snapshot = Snapshot {
            status: StatusCode::OK,
            headers: Default::default(),
            body: "x".into(),
        };
        for key in ["a", "b", "c"] {
            store.put(key.into(), snapshot.clone(), Duration::from_secs(5)).await;
            tokio::time::advance(Duration::from_secs(1)).await;
        }
        assert_eq!(store.len(), 2);
        assert!(store.get("a").await.is_none(), "the entry closest to expiry went first");
        assert!(store.get("c").await.is_some());
    }
}
