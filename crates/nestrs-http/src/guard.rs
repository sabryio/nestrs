use std::future::Future;
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::request::Parts;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::MethodRouter;

use crate::HttpError;

/// A pre-condition that runs before a route's handler.
///
/// Guards are ordinary injectable providers, so they can depend on other services
/// (a token verifier, a permissions service). They see the request head mutably:
/// authenticate, then stash the identity in `parts.extensions` for handlers to
/// extract. Return an [`HttpError`] to short-circuit with a response.
///
/// Reference them with `#[guard(MyGuard)]` on a controller or a route; the guard
/// must be a provider that the controller's module can see.
pub trait Guard: Send + Sync + 'static {
    /// Whether a request with no credentials is let through (an *optional* guard). Only
    /// documentation reads this: OpenAPI then lists the route's security as optional
    /// instead of required. Default `false`.
    const ANONYMOUS_OK: bool = false;

    /// Decides whether the request may proceed.
    fn check(&self, parts: &mut Parts) -> impl Future<Output = Result<(), HttpError>> + Send;
}

/// Wraps `route` so `guard` runs first. Used by `#[controller]`-generated code.
pub fn guarded<G: Guard>(route: MethodRouter, guard: Arc<G>) -> MethodRouter {
    route.route_layer(middleware::from_fn_with_state(guard, run_guard::<G>))
}

async fn run_guard<G: Guard>(State(guard): State<Arc<G>>, request: Request, next: Next) -> Response {
    let (mut parts, body) = request.into_parts();
    match guard.check(&mut parts).await {
        Ok(()) => next.run(Request::from_parts(parts, body)).await,
        Err(error) => error.into_response(),
    }
}

/// Like [`guarded`], for a hand-written axum [`Router`](axum::Router) merged with
/// `AppBuilder::merge_with`: every route in `router` runs `guard` first.
pub fn guarded_router(router: axum::Router, guard: Arc<impl Guard>) -> axum::Router {
    router.route_layer(middleware::from_fn_with_state(guard, run_guard::<_>))
}
