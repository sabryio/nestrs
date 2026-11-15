//! Token caching: a decorator that remembers an [`Authenticator`]'s verdicts.
//!
//! Worth it when verification is expensive, such as calling a token-introspection
//! endpoint or a database per request. Not needed for JWTs checked locally.
//!
//! ```ignore
//! #[provider]
//! fn authenticator(introspection: Arc<Introspection>) -> AuthenticatorPort {
//!     Arc::new(CachedAuthenticator::new(introspection, CacheConfig::default()))
//! }
//! ```
//!
//! ## Rules
//!
//! * Successes are kept for `ttl`. A **revoked token stays valid for up to `ttl`**, so
//!   keep it short (seconds to a minute) and shorter than the tolerance for revocation.
//! * `Invalid` and `Expired` verdicts are kept for the (shorter) `negative_ttl`, so
//!   a flood of one bad token costs one lookup. `Unavailable` is never cached: an outage
//!   must not outlive itself.
//! * Memory is bounded. When `capacity` live entries exist, new tokens are simply not
//!   cached (they are still verified), so an attacker cannot grow the table.
//! * Concurrent first requests with the same token each verify once; the cache is not a
//!   single-flight gate.
//!
//! This is the decorator pattern: the guard and controllers neither know nor care
//! whether the port they hold is cached (Liskov, open/closed).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use nestrs_core::BoxFuture;
use tokio::time::Instant;

use crate::Principal;
use crate::bearer::{AuthError, Authenticator, AuthenticatorPort};

/// How long and how much a [`CachedAuthenticator`] remembers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheConfig {
    /// How long a successful verification is reused. Zero disables positive caching.
    pub ttl: Duration,
    /// How long a rejection (`Invalid`, `Expired`) is reused. Zero disables it.
    pub negative_ttl: Duration,
    /// Most entries kept at once.
    pub capacity: usize,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            ttl: Duration::from_secs(30),
            negative_ttl: Duration::from_secs(5),
            capacity: 10_000,
        }
    }
}

struct Entry {
    verdict: Result<Principal, AuthError>,
    expires: Instant,
}

/// An [`Authenticator`] that consults `inner` only on a cache miss.
pub struct CachedAuthenticator {
    inner: AuthenticatorPort,
    config: CacheConfig,
    entries: Mutex<HashMap<String, Entry>>,
}

impl CachedAuthenticator {
    /// Caches `inner`'s verdicts according to `config`.
    pub fn new(inner: AuthenticatorPort, config: CacheConfig) -> Self {
        Self {
            inner,
            config,
            entries: Mutex::default(),
        }
    }

    fn entries(&self) -> MutexGuard<'_, HashMap<String, Entry>> {
        self.entries.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn lookup(&self, token: &str) -> Option<Result<Principal, AuthError>> {
        let mut entries = self.entries();
        match entries.get(token) {
            Some(entry) if entry.expires > Instant::now() => Some(entry.verdict.clone()),
            Some(_) => {
                entries.remove(token);
                None
            }
            None => None,
        }
    }

    fn remember(&self, token: &str, verdict: &Result<Principal, AuthError>) {
        let ttl = match verdict {
            Ok(_) => self.config.ttl,
            Err(AuthError::Invalid | AuthError::Expired) => self.config.negative_ttl,
            Err(AuthError::Unavailable(_)) => return,
        };
        if ttl.is_zero() {
            return;
        }
        let now = Instant::now();
        let mut entries = self.entries();
        if !entries.contains_key(token) && entries.len() >= self.config.capacity {
            entries.retain(|_, entry| entry.expires > now);
            if entries.len() >= self.config.capacity {
                tracing::warn!(capacity = self.config.capacity, "token cache is full; not caching");
                return;
            }
        }
        entries.insert(
            token.to_owned(),
            Entry {
                verdict: verdict.clone(),
                expires: now + ttl,
            },
        );
    }

    /// Forgets everything, e.g. after a key rotation or a mass revocation.
    pub fn clear(&self) {
        self.entries().clear();
    }

    /// Number of entries held, including expired ones not yet dropped.
    pub fn len(&self) -> usize {
        self.entries().len()
    }

    /// Whether nothing is cached.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl std::fmt::Debug for CachedAuthenticator {
    // Never prints the entries: they are keyed by live bearer tokens.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CachedAuthenticator")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl Authenticator for CachedAuthenticator {
    fn authenticate<'a>(&'a self, token: &'a str) -> BoxFuture<'a, Result<Principal, AuthError>> {
        Box::pin(async move {
            if let Some(verdict) = self.lookup(token) {
                return verdict;
            }
            let verdict = self.inner.authenticate(token).await;
            self.remember(token, &verdict);
            verdict
        })
    }
}

/// Convenience: a cached view of any authenticator, as the shared port.
pub fn cached(inner: impl Authenticator, config: CacheConfig) -> AuthenticatorPort {
    Arc::new(CachedAuthenticator::new(Arc::new(inner), config))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// Counts calls; accepts "good", rejects everything else, and fails on "down".
    #[derive(Default)]
    struct Counting {
        calls: AtomicUsize,
    }
    impl Authenticator for Counting {
        fn authenticate<'a>(&'a self, token: &'a str) -> BoxFuture<'a, Result<Principal, AuthError>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                match token {
                    "good" => Ok(Principal::new("ann")),
                    "old" => Err(AuthError::Expired),
                    "down" => Err(AuthError::Unavailable("introspection timeout".into())),
                    _ => Err(AuthError::Invalid),
                }
            })
        }
    }

    fn setup(config: CacheConfig) -> (Arc<Counting>, CachedAuthenticator) {
        let inner = Arc::new(Counting::default());
        (inner.clone(), CachedAuthenticator::new(inner, config))
    }
    fn calls(inner: &Counting) -> usize {
        inner.calls.load(Ordering::SeqCst)
    }

    #[tokio::test(start_paused = true)]
    async fn a_success_is_reused_until_the_ttl_passes() {
        let (inner, cache) = setup(CacheConfig::default());
        for _ in 0..5 {
            assert_eq!(cache.authenticate("good").await.unwrap().subject(), "ann");
        }
        assert_eq!(calls(&inner), 1);

        tokio::time::advance(Duration::from_secs(31)).await;
        cache.authenticate("good").await.unwrap();
        assert_eq!(calls(&inner), 2, "expired entries are verified again");
    }

    #[tokio::test(start_paused = true)]
    async fn rejections_are_cached_briefly() {
        let (inner, cache) = setup(CacheConfig::default());
        for token in ["nope", "old"] {
            for _ in 0..3 {
                assert!(cache.authenticate(token).await.is_err());
            }
        }
        assert_eq!(calls(&inner), 2, "one lookup per bad token");

        tokio::time::advance(Duration::from_secs(6)).await;
        assert_eq!(cache.authenticate("nope").await, Err(AuthError::Invalid));
        assert_eq!(
            calls(&inner),
            3,
            "the negative ttl (5 s) is shorter than the positive one"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn an_outage_is_never_cached() {
        let (inner, cache) = setup(CacheConfig::default());
        for _ in 0..3 {
            assert!(matches!(
                cache.authenticate("down").await,
                Err(AuthError::Unavailable(_))
            ));
        }
        assert_eq!(calls(&inner), 3);
        assert!(cache.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn zero_ttls_turn_caching_off() {
        let (inner, cache) = setup(CacheConfig {
            ttl: Duration::ZERO,
            negative_ttl: Duration::ZERO,
            capacity: 10,
        });
        cache.authenticate("good").await.unwrap();
        cache.authenticate("good").await.unwrap();
        assert_eq!(calls(&inner), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn the_cache_is_bounded_and_still_verifies_when_full() {
        let (inner, cache) = setup(CacheConfig {
            capacity: 2,
            ..CacheConfig::default()
        });
        for token in ["a", "b", "c"] {
            assert!(cache.authenticate(token).await.is_err());
        }
        assert_eq!(cache.len(), 2, "the third token was not cached");
        assert!(cache.authenticate("c").await.is_err());
        assert_eq!(calls(&inner), 4, "an uncached token is verified every time");

        tokio::time::advance(Duration::from_secs(6)).await;
        assert!(cache.authenticate("c").await.is_err());
        assert_eq!(cache.len(), 1, "expired entries make room again");
    }

    #[tokio::test(start_paused = true)]
    async fn clear_forgets_everything_and_debug_hides_tokens() {
        let (inner, cache) = setup(CacheConfig::default());
        cache.authenticate("good").await.unwrap();
        cache.clear();
        cache.authenticate("good").await.unwrap();
        assert_eq!(calls(&inner), 2);
        assert!(!format!("{cache:?}").contains("good"));
    }
}
