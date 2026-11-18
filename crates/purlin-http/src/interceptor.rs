use std::future::Future;
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::MethodRouter;

/// Wraps the execution of a route: code before the handler, code after it, or
/// instead of it (timing, caching, response mapping, retries).
///
/// Interceptors are injectable providers. Use `#[interceptor(MyInterceptor)]` on a
/// controller or a route, or give a value directly, `#[interceptor(RateLimit::per_minute(60))]`
/// (built once where the controller is mounted; not a provider, so it needs no
/// registration; on a controller it is shared by all its routes). They run *inside* the guards and around the handler:
/// `guards -> interceptors -> handler`, with the first one declared outermost.
///
/// ```ignore
/// impl Interceptor for Timing {
///     async fn intercept(&self, request: Request, next: Next) -> Response {
///         let started = Instant::now();
///         let mut response = next.run(request).await;
///         response.headers_mut().insert("x-elapsed-us", started.elapsed().as_micros().into());
///         response
///     }
/// }
/// ```
pub trait Interceptor: Send + Sync + 'static {
    /// Runs around `next`, which is the rest of the pipeline (inner interceptors,
    /// extractors, the handler).
    fn intercept(&self, request: Request, next: Next) -> impl Future<Output = Response> + Send;
}

/// Wraps `route` so `interceptor` runs around it. Used by `#[controller]`-generated code.
pub fn intercepted<I: Interceptor>(route: MethodRouter, interceptor: Arc<I>) -> MethodRouter {
    route.route_layer(middleware::from_fn_with_state(interceptor, run_interceptor::<I>))
}

/// Boxes an inline `#[interceptor(expr)]` value. The bound lives here so a value that
/// is not an [`Interceptor`] is reported on the expression the user wrote.
pub fn inline_interceptor<I: Interceptor>(interceptor: I) -> Arc<I> {
    Arc::new(interceptor)
}

async fn run_interceptor<I: Interceptor>(State(interceptor): State<Arc<I>>, request: Request, next: Next) -> Response {
    interceptor.intercept(request, next).await
}

/// Like [`intercepted`], for a hand-written axum [`Router`](axum::Router) merged with
/// `AppBuilder::merge_with`: every route in `router` runs inside `interceptor`.
pub fn intercepted_router<I: Interceptor>(router: axum::Router, interceptor: Arc<I>) -> axum::Router {
    router.route_layer(middleware::from_fn_with_state(interceptor, run_interceptor::<I>))
}
