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

use std::fmt;
use std::sync::Arc;

use crate::engine::key_of;
use crate::{Container, DiError, Token};

/// How long a provided value lives; the same type fx uses.
pub use fx::Lifetime;

/// The recipes for every non-singleton provider, kept after bootstrap.
pub struct ScopedProviders {
    scopes: fx::Scopes,
}

impl ScopedProviders {
    pub(crate) fn new(scopes: fx::Scopes) -> Self {
        Self { scopes }
    }

    /// How many request-scoped and transient providers there are.
    pub fn len(&self) -> usize {
        self.scopes.len()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.scopes.is_empty()
    }
}

impl fmt::Debug for ScopedProviders {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ScopedProviders({})", self.len())
    }
}

/// The container for one request, backed by an `fx` request scope.
///
/// Singletons come from the bootstrapped app, request-scoped values are built at most once per
/// scope, and transient values on every resolution.
pub struct RequestScope {
    inner: fx::RequestScope,
}

impl RequestScope {
    /// Opens a scope. The container argument is no longer needed (singletons live in the fx app)
    /// and is accepted for source compatibility.
    pub fn new(_container: Arc<Container>, providers: Arc<ScopedProviders>) -> Self {
        Self {
            inner: providers.scopes.open(),
        }
    }

    /// Supplies a value for this scope, such as the incoming request.
    pub fn seed<T>(&self, value: Arc<T>)
    where
        T: ?Sized + Send + Sync + 'static,
    {
        // Seeding an undeclared type always succeeds; a declared one fails only if it is not
        // request-scoped, which is a programming error reported when it is resolved.
        let _ = self.inner.seed_erased(key_of(Token::of::<T>()), Arc::new(value));
    }

    /// Resolves `T` from this scope.
    ///
    /// # Errors
    ///
    /// [`DiError::NotFound`] if nothing provides `T`, or the error a provider failed with.
    pub async fn resolve<T>(&self) -> Result<Arc<T>, DiError>
    where
        T: ?Sized + Send + Sync + 'static,
    {
        let token = Token::of::<T>();
        let stored = self
            .inner
            .resolve_erased(key_of(token))
            .await
            .map_err(|error| match error {
                fx::Error::Missing { .. } => DiError::NotFound { requested: token },
                other => crate::engine::into_di(other),
            })?;
        stored
            .downcast_ref::<Arc<T>>()
            .cloned()
            .ok_or(DiError::NotFound { requested: token })
    }
}

impl fmt::Debug for RequestScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RequestScope").field("inner", &self.inner).finish()
    }
}
