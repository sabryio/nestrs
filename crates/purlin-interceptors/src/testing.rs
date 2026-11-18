//! Helpers for the unit tests: a one-route router wrapped in the interceptor under test.

use std::sync::Arc;

use purlin_http::axum::Router;
use purlin_http::axum::body::{Body, to_bytes};
use purlin_http::axum::handler::Handler;
use purlin_http::axum::http::Request;
use purlin_http::axum::response::Response;
use purlin_http::axum::routing::{get as get_route, post as post_route};
use purlin_http::{Interceptor, intercepted};
use tower::ServiceExt;

pub fn router_with<I: Interceptor>(interceptor: I) -> Router {
    router_with_handler(interceptor, || async { "ok" })
}

pub fn router_with_handler<I, H, T>(interceptor: I, handler: H) -> Router
where
    I: Interceptor,
    H: Handler<T, ()>,
    T: 'static,
{
    Router::new().route("/", intercepted(get_route(handler), Arc::new(interceptor)))
}

pub fn router_with_echo<I: Interceptor>(interceptor: I) -> Router {
    Router::new().route(
        "/",
        intercepted(post_route(|body: String| async move { body }), Arc::new(interceptor)),
    )
}

pub async fn get(app: &Router, path: &str, headers: &[(&str, &str)]) -> Response {
    let mut request = Request::get(path);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    app.clone().oneshot(request.body(Body::empty()).unwrap()).await.unwrap()
}

pub async fn post(app: &Router, path: &str, body: &str) -> Response {
    post_with(app, path, &[], body).await
}

pub async fn post_with(app: &Router, path: &str, headers: &[(&str, &str)], body: &str) -> Response {
    let mut request = Request::post(path);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    app.clone()
        .oneshot(request.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap()
}

pub async fn text(response: Response) -> String {
    String::from_utf8(to_bytes(response.into_body(), usize::MAX).await.unwrap().to_vec()).unwrap()
}
