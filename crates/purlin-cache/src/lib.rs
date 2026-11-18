//! # purlin-cache
//!
//! Read-through caching for *application services*, one level below the HTTP
//! `ResponseCache` interceptor: cache what a use case or repository computes, not what a
//! route returns, so every caller (HTTP, jobs, queue handlers) benefits.
//!
//! ```ignore
//! #[injectable]
//! struct CachedPosts { inner: Arc<PostsRepoPort>, cache: Arc<PostCache> }
//!
//! #[async_trait]
//! impl PostsRepo for CachedPosts {            // a decorator: same port, same contract
//!     async fn find(&self, id: Uuid) -> Result<Option<Post>, RepoError> {
//!         self.cache.get_or_load(&id.to_string(), || self.inner.find(id)).await
//!     }
//!     async fn save(&self, post: Post) -> Result<(), RepoError> {
//!         self.inner.save(post.clone()).await?;
//!         self.cache.invalidate(&post.id.to_string()).await;   // writes keep reads honest
//!         Ok(())
//!     }
//! }
//! ```
//!
//! ## Guarantees
//!
//! * **Single flight per key per process**: concurrent misses for one key run the loader
//!   once; the others wait and read the stored result.
//! * **Fail open**: a store that errors is a miss (and a write that fails is logged), so a
//!   cache outage costs speed, never availability. Loader errors are returned as they are
//!   and never cached.
//! * **Namespaced keys** (`<namespace>:<key>`) let one store serve many caches.
//!
//! ## Design
//!
//! * [`ValueStore`] is a tiny port (get / put / remove of JSON text) that memory and Redis
//!   adapters implement identically (LSP, ISP).
//! * [`ServiceCache`] holds the policy (ttl, namespace, single flight) and knows nothing
//!   about where values live (SRP, DIP). New backends need no change here (OCP).

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use purlin::prelude::*;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::time::Instant;

/// The store failed. Callers of [`ServiceCache`] never see it (it fails open); adapters
/// return it so the failure is logged with its reason.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("cache store unavailable: {0}")]
pub struct StoreError(pub String);

/// Port: expiring key → JSON text.
///
/// Contract: `get` never returns an expired value; `put` replaces any existing one and
/// the value disappears after `ttl`; `remove` of a missing key succeeds.
#[port]
pub trait ValueStore {
    /// The live value for `key`.
    async fn get(&self, key: &str) -> Result<Option<String>, StoreError>;
    /// Stores `value` for `ttl`.
    async fn put(&self, key: &str, value: String, ttl: Duration) -> Result<(), StoreError>;
    /// Forgets `key`.
    async fn remove(&self, key: &str) -> Result<(), StoreError>;
}

/// A bounded in-process [`ValueStore`]. When full it drops expired entries, then the one
/// closest to expiry.
#[derive(Debug)]
pub struct MemoryValueStore {
    capacity: usize,
    entries: Mutex<HashMap<String, (String, Instant)>>,
}

impl MemoryValueStore {
    /// Holds at most `capacity` entries.
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            entries: Mutex::default(),
        }
    }
}

#[async_trait]
impl ValueStore for MemoryValueStore {
    async fn get(&self, key: &str) -> Result<Option<String>, StoreError> {
        let now = Instant::now();
        let mut entries = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        match entries.get(key) {
            Some((value, until)) if *until > now => Ok(Some(value.clone())),
            Some(_) => {
                entries.remove(key);
                Ok(None)
            }
            None => Ok(None),
        }
    }

    async fn put(&self, key: &str, value: String, ttl: Duration) -> Result<(), StoreError> {
        let now = Instant::now();
        let mut entries = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        if entries.len() >= self.capacity && !entries.contains_key(key) {
            entries.retain(|_, (_, until)| *until > now);
            if entries.len() >= self.capacity {
                let soonest = entries
                    .iter()
                    .min_by_key(|(_, (_, until))| *until)
                    .map(|(k, _)| k.clone());
                if let Some(soonest) = soonest {
                    entries.remove(&soonest);
                }
            }
        }
        entries.insert(key.to_owned(), (value, now + ttl));
        Ok(())
    }

    async fn remove(&self, key: &str) -> Result<(), StoreError> {
        self.entries.lock().unwrap_or_else(|p| p.into_inner()).remove(key);
        Ok(())
    }
}

/// A read-through cache with a namespace and a time to live.
pub struct ServiceCache {
    store: ValueStorePort,
    namespace: String,
    ttl: Duration,
    flights: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl ServiceCache {
    /// A cache over `store`; keys are prefixed `namespace:` and live for `ttl`.
    pub fn new(store: ValueStorePort, namespace: impl Into<String>, ttl: Duration) -> Self {
        Self {
            store,
            namespace: namespace.into(),
            ttl,
            flights: Mutex::default(),
        }
    }

    fn full_key(&self, key: &str) -> String {
        format!("{}:{key}", self.namespace)
    }

    async fn lookup<T: DeserializeOwned>(&self, full: &str) -> Option<T> {
        match self.store.get(full).await {
            Ok(Some(text)) => match serde_json::from_str(&text) {
                Ok(value) => Some(value),
                Err(error) => {
                    // A value this version cannot read (a deploy changed the type): a miss.
                    tracing::warn!(key = %full, %error, "ignoring an unreadable cache entry");
                    None
                }
            },
            Ok(None) => None,
            Err(error) => {
                tracing::warn!(key = %full, %error, "cache read failed; treating it as a miss");
                None
            }
        }
    }

    /// The cached value for `key`, or the result of `load` (stored for next time).
    /// Concurrent calls for one key run `load` once.
    pub async fn get_or_load<T, E, F, Fut>(&self, key: &str, load: F) -> Result<T, E>
    where
        T: Serialize + DeserializeOwned,
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        let full = self.full_key(key);
        if let Some(hit) = self.lookup(&full).await {
            return Ok(hit);
        }

        let gate = self
            .flights
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .entry(full.clone())
            .or_default()
            .clone();
        let _turn = gate.lock().await;
        let outcome = async {
            // Whoever held the gate before us may have filled it.
            if let Some(hit) = self.lookup(&full).await {
                return Ok(hit);
            }
            let value = load().await?;
            match serde_json::to_string(&value) {
                Ok(text) => {
                    if let Err(error) = self.store.put(&full, text, self.ttl).await {
                        tracing::warn!(key = %full, %error, "cache write failed");
                    }
                }
                Err(error) => tracing::warn!(key = %full, %error, "value is not cacheable"),
            }
            Ok(value)
        }
        .await;

        // Forget the gate once nobody else is queued behind it (ours + the map's = 2).
        let mut flights = self.flights.lock().unwrap_or_else(|p| p.into_inner());
        if flights.get(&full).is_some_and(|entry| Arc::strong_count(entry) <= 2) {
            flights.remove(&full);
        }
        outcome
    }

    /// Forgets `key`; call it after the data behind it changes.
    pub async fn invalidate(&self, key: &str) {
        let full = self.full_key(key);
        if let Err(error) = self.store.remove(&full).await {
            tracing::warn!(key = %full, %error, "cache invalidation failed; the entry expires on its own");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    fn cache(ttl: Duration) -> ServiceCache {
        ServiceCache::new(Arc::new(MemoryValueStore::new(16)), "posts", ttl)
    }

    #[tokio::test]
    async fn loads_once_then_serves_from_the_cache() {
        let cache = cache(Duration::from_secs(60));
        let calls = AtomicU32::new(0);
        for _ in 0..3 {
            let value: u32 = cache
                .get_or_load("a", || async {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, ()>(7)
                })
                .await
                .unwrap();
            assert_eq!(value, 7);
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn errors_are_returned_and_not_cached() {
        let cache = cache(Duration::from_secs(60));
        let failed: Result<u32, &str> = cache.get_or_load("a", || async { Err("db down") }).await;
        assert_eq!(failed, Err("db down"));
        let ok: Result<u32, &str> = cache.get_or_load("a", || async { Ok(1) }).await;
        assert_eq!(ok, Ok(1), "the failure left nothing behind");
    }

    #[tokio::test(start_paused = true)]
    async fn entries_expire_and_invalidate_forgets() {
        let cache = cache(Duration::from_secs(10));
        let calls = AtomicU32::new(0);
        let load = || async { Ok::<_, ()>(calls.fetch_add(1, Ordering::SeqCst)) };
        assert_eq!(cache.get_or_load("a", load).await, Ok(0));
        assert_eq!(cache.get_or_load("a", load).await, Ok(0));
        tokio::time::advance(Duration::from_secs(11)).await;
        assert_eq!(cache.get_or_load("a", load).await, Ok(1), "expired");
        cache.invalidate("a").await;
        assert_eq!(cache.get_or_load("a", load).await, Ok(2), "invalidated");
    }

    #[tokio::test]
    async fn concurrent_misses_share_one_load() {
        let cache = Arc::new(cache(Duration::from_secs(60)));
        let calls = Arc::new(AtomicU32::new(0));
        let tasks: Vec<_> = (0..8)
            .map(|_| {
                let (cache, calls) = (cache.clone(), calls.clone());
                tokio::spawn(async move {
                    cache
                        .get_or_load("hot", || async {
                            calls.fetch_add(1, Ordering::SeqCst);
                            tokio::time::sleep(Duration::from_millis(50)).await;
                            Ok::<_, ()>(42u32)
                        })
                        .await
                })
            })
            .collect();
        for task in tasks {
            assert_eq!(task.await.unwrap(), Ok(42));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(cache.flights.lock().unwrap().is_empty(), "gates are cleaned up");
    }

    struct Broken;

    #[async_trait]
    impl ValueStore for Broken {
        async fn get(&self, _: &str) -> Result<Option<String>, StoreError> {
            Err(StoreError("down".into()))
        }
        async fn put(&self, _: &str, _: String, _: Duration) -> Result<(), StoreError> {
            Err(StoreError("down".into()))
        }
        async fn remove(&self, _: &str) -> Result<(), StoreError> {
            Err(StoreError("down".into()))
        }
    }

    #[tokio::test]
    async fn a_broken_store_only_costs_speed() {
        let cache = ServiceCache::new(Arc::new(Broken), "x", Duration::from_secs(5));
        let value: Result<u32, ()> = cache.get_or_load("a", || async { Ok(9) }).await;
        assert_eq!(value, Ok(9));
        cache.invalidate("a").await;
    }
}
