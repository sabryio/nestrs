use std::future::Future;
use std::sync::Arc;

use axum::Router;
use axum::extract::{Request, State};
use axum::middleware::{self, Next};
use axum::response::Response;
use nestrs_core::BoxFuture;

use crate::HttpError;
use crate::scope::RequestInfo;

/// Put on responses built from an [`HttpError`] so [`ExceptionFilter`]s can see the
/// error rather than only the rendered body.
#[derive(Debug, Clone)]
pub(crate) struct CaughtError(pub(crate) Arc<HttpError>);

/// Inspects every [`HttpError`] on its way out and may replace the response
/// (a different error format, localisation, alerting, redaction).
///
/// Filters are injectable providers registered on the app with
/// `App::builder(..).filter::<MyFilter>()`. They see errors from guards, pipes,
/// extractors and handlers alike. The first filter returning `Some(response)` wins;
/// returning `None` leaves the response as it is, so a filter can also just observe.
pub trait ExceptionFilter: Send + Sync + 'static {
    /// Handles `error`, raised while serving `request`.
    fn catch(&self, error: &HttpError, request: &RequestInfo) -> impl Future<Output = Option<Response>> + Send;
}

type FilterFn = dyn for<'a> Fn(&'a HttpError, &'a RequestInfo) -> BoxFuture<'a, Option<Response>> + Send + Sync;

/// A filter with its type erased so several can live in one list.
#[derive(Clone)]
pub(crate) struct ErasedFilter(Arc<FilterFn>);

impl ErasedFilter {
    pub(crate) fn new<F: ExceptionFilter>(filter: Arc<F>) -> Self {
        Self(Arc::new(move |error, request| {
            let filter = Arc::clone(&filter);
            Box::pin(async move { filter.catch(error, request).await })
        }))
    }
}

/// Wraps `router` so the filters see every error response.
pub(crate) fn with_filters(router: Router, filters: Vec<ErasedFilter>) -> Router {
    if filters.is_empty() {
        return router;
    }
    router.layer(middleware::from_fn_with_state(Arc::new(filters), run_filters))
}

async fn run_filters(State(filters): State<Arc<Vec<ErasedFilter>>>, request: Request, next: Next) -> Response {
    let info = RequestInfo {
        method: request.method().clone(),
        uri: request.uri().clone(),
        headers: request.headers().clone(),
    };
    let response = next.run(request).await;
    let Some(CaughtError(error)) = response.extensions().get::<CaughtError>().cloned() else {
        return response;
    };
    for filter in filters.iter() {
        if let Some(replacement) = (filter.0)(&error, &info).await {
            return replacement;
        }
    }
    response
}
