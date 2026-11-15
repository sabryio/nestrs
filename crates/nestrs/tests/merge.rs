//! `AppBuilder::merge`: hand-written axum routes next to controllers.

use std::sync::Arc as StdArc;
use std::sync::atomic::{AtomicUsize, Ordering};

use http_body_util::BodyExt;
use nestrs::http::axum::Router;
use nestrs::http::axum::body::Body;
use nestrs::http::axum::extract::{Request, State};
use nestrs::http::axum::http::{HeaderValue, Request as HttpRequest};
use nestrs::http::axum::middleware::Next;
use nestrs::http::axum::response::{IntoResponse, Response};
use nestrs::http::axum::routing::get;
use nestrs::http::{Parts, guarded_router, intercepted_router};
use nestrs::prelude::*;
use tower::ServiceExt;

#[injectable]
struct Greeter;

impl Greeter {
    fn greet(&self) -> &'static str {
        "hello from the container"
    }
}

#[injectable]
struct HelloController {
    greeter: Arc<Greeter>,
}

#[controller("/hello")]
impl HelloController {
    #[get]
    async fn hello(&self) -> String {
        self.greeter.greet().to_owned()
    }
}

#[module(providers = [Greeter], controllers = [HelloController])]
struct AppModule;

/// Real axum state, private to the merged router.
#[derive(Clone, Default)]
struct Counter(StdArc<AtomicUsize>);

async fn raw_count(State(counter): State<Counter>) -> String {
    (counter.0.fetch_add(1, Ordering::SeqCst) + 1).to_string()
}

async fn raw_inject(Inject(greeter): Inject<Greeter>) -> String {
    greeter.greet().to_owned()
}

async fn raw_conflict() -> Result<String, HttpError> {
    Err(HttpError::conflict("raw handler says no"))
}

fn raw_router() -> Router {
    Router::new()
        .route("/raw/count", get(raw_count))
        .with_state(Counter::default())
        .route("/raw/inject", get(raw_inject))
        .route("/raw/conflict", get(raw_conflict))
}

/// A filter that turns every 409 into a 418, to prove filters reach merged routes.
#[injectable]
struct Teapot;

impl ExceptionFilter for Teapot {
    async fn catch(&self, error: &HttpError, _: &RequestInfo) -> Option<Response> {
        matches!(error, HttpError::Conflict(_)).then(|| StatusCode::IM_A_TEAPOT.into_response())
    }
}

#[module(providers = [Greeter, Teapot], controllers = [HelloController])]
struct FilteredModule;

struct Stamp;

impl Interceptor for Stamp {
    async fn intercept(&self, request: Request, next: Next) -> Response {
        let mut response = next.run(request).await;
        response
            .headers_mut()
            .insert("x-stamp", HeaderValue::from_static("yes"));
        response
    }
}

async fn call(router: &Router, path: &str) -> (StatusCode, Option<String>, String) {
    let response = router
        .clone()
        .oneshot(HttpRequest::get(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let stamp = response
        .headers()
        .get("x-stamp")
        .map(|v| v.to_str().unwrap().to_owned());
    let body = String::from_utf8(response.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
    (status, stamp, body)
}

#[tokio::test]
async fn raw_axum_state_works_next_to_controllers() {
    let app = App::builder(AppModule).merge(raw_router()).build().await.unwrap();
    let router = app.router();
    assert_eq!(call(&router, "/raw/count").await.2, "1");
    assert_eq!(
        call(&router, "/raw/count").await.2,
        "2",
        "the axum state is shared across requests"
    );
    assert_eq!(call(&router, "/hello").await.2, "hello from the container");
}

#[tokio::test]
async fn merged_handlers_can_take_inject() {
    let app = App::builder(AppModule).merge(raw_router()).build().await.unwrap();
    let (status, _, body) = call(&app.router(), "/raw/inject").await;
    assert_eq!((status, body.as_str()), (StatusCode::OK, "hello from the container"));
}

#[tokio::test]
async fn exception_filters_reach_merged_routes() {
    let app = App::builder(FilteredModule)
        .filter::<Teapot>()
        .merge(raw_router())
        .build()
        .await
        .unwrap();
    assert_eq!(call(&app.router(), "/raw/conflict").await.0, StatusCode::IM_A_TEAPOT);
}

#[tokio::test]
async fn prefix_and_app_wide_interceptors_cover_merged_routes() {
    let app = App::builder(AppModule)
        .merge(raw_router())
        .build()
        .await
        .unwrap()
        .global_prefix("/api")
        .interceptor(Stamp);
    let router = app.router();
    for path in ["/api/raw/count", "/api/hello"] {
        let (status, stamp, _) = call(&router, path).await;
        assert_eq!((status, stamp.as_deref()), (StatusCode::OK, Some("yes")), "{path}");
    }
    assert_eq!(
        call(&router, "/raw/count").await.0,
        StatusCode::NOT_FOUND,
        "the prefix moved it"
    );
}

#[tokio::test]
async fn unknown_routes_keep_the_json_404_even_if_the_merged_router_has_a_fallback() {
    let custom = Router::new()
        .route("/raw", get(|| async { "x" }))
        .fallback(|| async { "custom fallback" });
    let app = App::builder(AppModule).merge(custom).build().await.unwrap();
    let (status, _, body) = call(&app.router(), "/nope").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains("\"code\":\"not_found\""), "{body}");
}

#[tokio::test]
#[should_panic(expected = "Overlapping method route")]
async fn a_path_that_collides_with_a_controller_route_fails_at_build() {
    let clash = Router::new().route("/hello", get(|| async { "x" }));
    let _ = App::builder(AppModule).merge(clash).build().await;
}

/// A guard for raw routes: needs `x-pass`.
#[injectable]
struct NeedsPass;

impl Guard for NeedsPass {
    async fn check(&self, parts: &mut Parts) -> Result<(), HttpError> {
        match parts.headers.contains_key("x-pass") {
            true => Ok(()),
            false => Err(HttpError::unauthorized("no pass")),
        }
    }
}

#[module(providers = [Greeter, NeedsPass], controllers = [HelloController])]
struct GuardedModule;

async fn call_with(router: &Router, path: &str, header: Option<(&str, &str)>) -> StatusCode {
    let mut request = HttpRequest::get(path);
    if let Some((name, value)) = header {
        request = request.header(name, value);
    }
    router
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn merge_with_puts_the_apps_own_guard_and_interceptor_on_raw_routes() {
    let app = App::builder(GuardedModule)
        .merge_with(|container| {
            let guard = container.resolve::<NeedsPass>()?;
            let admin = Router::new().route("/admin/reload", get(|| async { "reloaded" }));
            let admin = intercepted_router(admin, StdArc::new(Stamp));
            Ok(guarded_router(admin, guard))
        })
        .merge(raw_router())
        .build()
        .await
        .unwrap();
    let router = app.router();

    assert_eq!(
        call_with(&router, "/admin/reload", None).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call_with(&router, "/admin/reload", Some(("x-pass", "1"))).await,
        StatusCode::OK
    );
    let (_, stamp, _) = call(&router, "/admin/reload").await;
    assert_eq!(stamp, None, "the guard answered before the interceptor ran");
    assert_eq!(
        call(&router, "/raw/count").await.0,
        StatusCode::OK,
        "other merged routes are not guarded"
    );
}

#[tokio::test]
async fn merge_with_can_hand_a_provider_to_a_raw_handler_as_axum_state() {
    let app = App::builder(AppModule)
        .merge_with(|container| {
            let greeter = container.resolve::<Greeter>()?;
            Ok(Router::new()
                .route(
                    "/raw/svc",
                    get(|State(g): State<Arc<Greeter>>| async move { g.greet() }),
                )
                .with_state(greeter))
        })
        .build()
        .await
        .unwrap();
    assert_eq!(call(&app.router(), "/raw/svc").await.2, "hello from the container");
}

#[tokio::test]
async fn a_failed_resolution_in_merge_with_fails_the_build() {
    struct Unknown;
    let result = App::builder(AppModule)
        .merge_with(|container| {
            container.resolve::<Unknown>()?;
            Ok(Router::new())
        })
        .build()
        .await;
    assert!(result.is_err());
}
