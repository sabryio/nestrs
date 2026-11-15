//! Request scopes over HTTP: the layer that creates one per request, the
//! [`Inject`] extractor that resolves from it, and [`HttpModule`].

use std::ops::Deref;
use std::sync::Arc;

use axum::Router;
use axum::extract::{FromRequestParts, Request, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, Method, Uri};
use axum::middleware::{self, Next};
use axum::response::Response;
use nestrs_core::{Container, ModuleDef, ModuleRef, Provider, RequestScope, ScopedProviders, Token};

use crate::HttpError;

/// What a request-scoped provider can learn about the request it serves.
///
/// Seeded into every [`RequestScope`]; depend on it from a request-scoped
/// provider (`Arc<RequestInfo>` in an `#[injectable]`) after importing [`HttpModule`].
#[derive(Debug, Clone)]
pub struct RequestInfo {
    /// The request method.
    pub method: Method,
    /// The request URI.
    pub uri: Uri,
    /// The request headers.
    pub headers: HeaderMap,
}

/// Declares [`RequestInfo`] so request-scoped providers can depend on it. Global:
/// import it once in the root module.
pub struct HttpModule;

impl HttpModule {
    /// The module definition to import (`imports = [HttpModule::module()]`).
    pub fn module() -> ModuleRef {
        ModuleDef::new("HttpModule")
            .global()
            .providers([Provider::seeded::<RequestInfo>()])
            .exports([Token::of::<RequestInfo>()])
            .into_ref()
    }
}

#[derive(Clone)]
struct ScopeState {
    container: Arc<Container>,
    providers: Arc<ScopedProviders>,
}

/// Wraps `router` so every request gets its own [`RequestScope`].
pub(crate) fn with_request_scopes(
    router: Router,
    container: Arc<Container>,
    providers: Arc<ScopedProviders>,
) -> Router {
    let state = ScopeState { container, providers };
    router.layer(middleware::from_fn_with_state(state, open_scope))
}

async fn open_scope(State(state): State<ScopeState>, mut request: Request, next: Next) -> Response {
    let scope = Arc::new(RequestScope::new(state.container, state.providers));
    scope.seed(Arc::new(RequestInfo {
        method: request.method().clone(),
        uri: request.uri().clone(),
        headers: request.headers().clone(),
    }));
    request.extensions_mut().insert(scope);
    next.run(request).await
}

/// Resolves `T` from the request's scope: a singleton, this request's instance of a
/// request-scoped provider, or a fresh transient one.
///
/// ```ignore
/// #[get]
/// async fn list(&self, Inject(ctx): Inject<RequestContext>) -> .. { .. }
/// ```
///
/// `#[controller]` notices `Inject<T>` parameters and checks at startup that `T` is
/// provided and visible to the controller's module.
#[derive(Debug)]
pub struct Inject<T: ?Sized>(pub Arc<T>);

impl<T: ?Sized> Deref for Inject<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

impl<S, T> FromRequestParts<S> for Inject<T>
where
    S: Send + Sync,
    T: ?Sized + Send + Sync + 'static,
{
    type Rejection = HttpError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        scope_of(parts)?
            .resolve::<T>()
            .await
            .map(Self)
            .map_err(|error| HttpError::internal(error.to_string()))
    }
}

/// The scope the layer opened for this request.
pub(crate) fn scope_of(parts: &Parts) -> Result<Arc<RequestScope>, HttpError> {
    parts
        .extensions
        .get::<Arc<RequestScope>>()
        .cloned()
        .ok_or_else(|| HttpError::internal("no request scope: the request did not pass through `App`"))
}
