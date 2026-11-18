use purlin::http::axum::response::Response;
use purlin::prelude::*;

/// Exception filter: logs every server-side (5xx) failure with its request. It only
/// observes (returns `None`), so the response the client gets is unchanged.
///
/// [`Service::run`](crate::Service::run) registers it for you; to register it by hand
/// use `App::builder(AppModule).filter::<ErrorLogFilter>()`.
#[injectable]
pub struct ErrorLogFilter;

impl ExceptionFilter for ErrorLogFilter {
    async fn catch(&self, error: &HttpError, request: &RequestInfo) -> Option<Response> {
        if error.status().is_server_error() {
            tracing::error!(
                status = error.status().as_u16(),
                method = %request.method,
                path = request.uri.path(),
                request_id = request.headers.get("x-request-id").and_then(|v| v.to_str().ok()),
                %error,
                "request failed"
            );
        }
        None
    }
}
