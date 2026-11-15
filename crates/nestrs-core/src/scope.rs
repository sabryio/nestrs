//! Provider lifetimes and the per-request container.
//!
//! Most providers are singletons, built once at bootstrap. Two shorter lifetimes
//! exist for state that must not be shared:
//!
//! - **Request**: built lazily, at most once per [`RequestScope`] (one HTTP request),
//!   then reused for the rest of that request.
//! - **Transient**: built anew on every resolution.
//!
//! Rule, checked at startup: *only a non-singleton may depend on a non-singleton*.
//! A singleton that held a request-scoped value would silently pin the first
//! request's state forever, so the graph rejects it with [`DiError::ScopeMismatch`].

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};

use crate::container::Stored;
use crate::provider::BoxFuture;
use crate::{Container, DiError, Provider, Token};

/// How long a provider's instance lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lifetime {
    /// One instance for the whole application (the default).
    Singleton,
    /// One instance per request scope, built on first use.
    Request,
    /// A new instance every time it is resolved.
    Transient,
}

impl fmt::Display for Lifetime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Singleton => "singleton",
            Self::Request => "request-scoped",
            Self::Transient => "transient",
        })
    }
}

/// The recipes for every non-singleton provider, kept after bootstrap.
#[derive(Debug, Default)]
pub struct ScopedProviders {
    providers: HashMap<Token, Provider>,
}

impl ScopedProviders {
    pub(crate) fn new(providers: HashMap<Token, Provider>) -> Self {
        Self { providers }
    }

    /// How many request-scoped and transient providers exist.
    pub fn len(&self) -> usize {
        self.providers.len()
    }

    /// Whether the application has no non-singleton providers.
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }
}

/// The container for one request: resolves singletons from the application
/// container and builds request-scoped and transient providers on demand.
///
/// Dependencies of a scoped provider are resolved (recursively, async) into a small
/// overlay container, and the provider's ordinary `Injectable::build` runs against
/// it, so `#[injectable]` types work unchanged in any lifetime.
pub struct RequestScope {
    container: Arc<Container>,
    providers: Arc<ScopedProviders>,
    cache: Mutex<HashMap<Token, Stored>>,
}

impl RequestScope {
    /// Creates an empty scope over the application's container.
    pub fn new(container: Arc<Container>, providers: Arc<ScopedProviders>) -> Self {
        Self {
            container,
            providers,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Supplies the value of a [`Provider::seeded`] provider for this scope.
    pub fn seed<T>(&self, value: Arc<T>)
    where
        T: ?Sized + Send + Sync + 'static,
    {
        self.lock().insert(Token::of::<T>(), Arc::new(value));
    }

    /// Resolves `T`: a singleton, this scope's instance, or a fresh transient one.
    pub async fn resolve<T>(&self) -> Result<Arc<T>, DiError>
    where
        T: ?Sized + Send + Sync + 'static,
    {
        let token = Token::of::<T>();
        let stored = self.resolve_stored(token).await?;
        stored
            .downcast_ref::<Arc<T>>()
            .cloned()
            .ok_or(DiError::NotFound { requested: token })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<Token, Stored>> {
        self.cache.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn resolve_stored(&self, token: Token) -> BoxFuture<'_, Result<Stored, DiError>> {
        Box::pin(async move {
            if let Some(hit) = self.lock().get(&token) {
                return Ok(Arc::clone(hit));
            }
            let Some(provider) = self.providers.providers.get(&token) else {
                return self
                    .container
                    .get_raw(token)
                    .ok_or(DiError::NotFound { requested: token });
            };

            let mut overlay = Container::new();
            for dependency in provider.dependencies() {
                overlay.insert_raw(*dependency, self.resolve_stored(*dependency).await?);
            }
            let built = provider.build(&overlay).await?;

            match provider.lifetime() {
                // Keep the first instance if a concurrent resolution got there first.
                Lifetime::Request => Ok(Arc::clone(self.lock().entry(token).or_insert(built))),
                _ => Ok(built),
            }
        })
    }
}

impl fmt::Debug for RequestScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RequestScope")
            .field("cached", &self.lock().len())
            .finish_non_exhaustive()
    }
}
