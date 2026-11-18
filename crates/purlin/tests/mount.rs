//! `Mount`: raw routes with guards, interceptors and OpenAPI docs, applied like a controller's.

use std::sync::{Arc as StdArc, Mutex};

use http_body_util::BodyExt;
use purlin::http::axum::Router;
use purlin::http::axum::body::Body;
use purlin::http::axum::extract::Request;
use purlin::http::axum::http::{HeaderValue, Request as HttpRequest};
use purlin::http::axum::middleware::Next;
use purlin::http::axum::response::Response;
use purlin::http::axum::routing::{get, post};
use purlin::http::{Mount, Parts};
use purlin::openapi::{ApiInfo, BodyDoc, ResponseDoc, RouteDoc, ToSchema};
use purlin::prelude::*;
use serde::Deserialize;
use serde_json::Value;
use tower::ServiceExt;

/// Records the order things run in.
type Trace = StdArc<Mutex<Vec<&'static str>>>;

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

#[injectable]
struct Optional;

impl Guard for Optional {
    const ANONYMOUS_OK: bool = true;
    async fn check(&self, _: &mut Parts) -> Result<(), HttpError> {
        Ok(())
    }
}

struct Stamp(&'static str, Trace);

impl Interceptor for Stamp {
    async fn intercept(&self, request: Request, next: Next) -> Response {
        self.1.lock().unwrap().push(self.0);
        let mut response = next.run(request).await;
        response
            .headers_mut()
            .insert("x-stamp", HeaderValue::from_static("yes"));
        response
    }
}

#[derive(Deserialize, ToSchema)]
struct Hook {
    #[allow(dead_code)]
    name: String,
}

#[module(providers = [NeedsPass, Optional])]
struct AppModule;

fn routes(trace: Trace) -> Router {
    Router::new()
        .route(
            "/hooks/run",
            get(move || {
                let trace = trace.clone();
                async move {
                    trace.lock().unwrap().push("handler");
                    "ran"
                }
            }),
        )
        .route("/hooks/new", post(|| async { "created" }))
}

async fn app_with(mount: Mount) -> App {
    App::builder(AppModule).mount(mount).build().await.unwrap()
}

async fn call(app: &App, path: &str, pass: bool) -> (u16, bool, String) {
    let mut request = HttpRequest::get(path);
    if pass {
        request = request.header("x-pass", "1");
    }
    let response = app
        .router()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status().as_u16();
    let stamped = response.headers().contains_key("x-stamp");
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, stamped, String::from_utf8_lossy(&body).into_owned())
}

#[tokio::test]
async fn a_guard_protects_every_mounted_route() {
    let app = app_with(Mount::new(routes(Trace::default())).guard::<NeedsPass>()).await;
    assert_eq!(call(&app, "/hooks/run", false).await.0, 401);
    assert_eq!(call(&app, "/hooks/run", true).await, (200, false, "ran".into()));
}

#[tokio::test]
async fn guards_run_before_interceptors_whatever_the_call_order() {
    let trace = Trace::default();
    // Interceptor written first, guard last: the guard must still be outermost.
    let app = app_with(
        Mount::new(routes(trace.clone()))
            .interceptor(Stamp("stamp", trace.clone()))
            .guard::<NeedsPass>(),
    )
    .await;

    let (status, stamped, _) = call(&app, "/hooks/run", false).await;
    assert_eq!(
        (status, stamped),
        (401, false),
        "a rejected request never reaches the interceptor"
    );
    assert!(trace.lock().unwrap().is_empty());

    let (status, stamped, _) = call(&app, "/hooks/run", true).await;
    assert_eq!((status, stamped), (200, true));
    assert_eq!(
        *trace.lock().unwrap(),
        ["stamp", "handler"],
        "interceptor, then handler"
    );
}

#[tokio::test]
async fn the_last_interceptor_added_is_outermost() {
    let trace = Trace::default();
    let app = app_with(
        Mount::new(routes(trace.clone()))
            .interceptor(Stamp("inner", trace.clone()))
            .interceptor(Stamp("outer", trace.clone())),
    )
    .await;
    call(&app, "/hooks/run", false).await;
    assert_eq!(*trace.lock().unwrap(), ["outer", "inner", "handler"]);
}

async fn spec(app: App) -> Value {
    serde_json::to_value(app.global_prefix("/api").openapi(&ApiInfo::new("Hooks", "1"))).unwrap()
}

#[tokio::test]
async fn documented_mounts_appear_in_openapi_with_the_prefix_and_security() {
    let mount = Mount::new(routes(Trace::default()))
        .guard::<NeedsPass>()
        .document(RouteDoc::get("/hooks/run", "run_hook").tagged("Hooks").docs(&[
            "Runs the hook.",
            "",
            "Synchronously.",
        ]))
        .document(
            RouteDoc::post("/hooks/new", "new_hook")
                .tagged("Hooks")
                .request_body(Some(BodyDoc::of::<Hook>()))
                .response(ResponseDoc::new(201, None)),
        );
    let spec = spec(app_with(mount).await).await;

    let run = &spec["paths"]["/api/hooks/run"]["get"];
    assert_eq!(run["summary"], "Runs the hook.");
    assert_eq!(run["operationId"], "Hooks_run_hook");
    assert_eq!(run["tags"][0], "Hooks");
    assert!(
        run["security"].is_array(),
        "a guard on the mount marks its documented routes secured: {run}"
    );
    assert!(run["responses"]["401"].is_object());

    let new = &spec["paths"]["/api/hooks/new"]["post"];
    assert!(
        new["requestBody"]["content"]["application/json"]["schema"]["$ref"]
            .as_str()
            .unwrap()
            .ends_with("/Hook")
    );
    assert!(spec["components"]["schemas"]["Hook"].is_object());
}

#[tokio::test]
async fn an_all_optional_guard_documents_anonymous_access() {
    let mount = Mount::new(routes(Trace::default()))
        .guard::<Optional>()
        .document(RouteDoc::get("/hooks/run", "run_hook"));
    let spec = spec(app_with(mount).await).await;
    let security = spec["paths"]["/api/hooks/run"]["get"]["security"]
        .as_array()
        .unwrap()
        .clone();
    assert!(
        security
            .iter()
            .any(|entry| entry.as_object().is_some_and(|o| o.is_empty())),
        "{security:?}"
    );
}

#[tokio::test]
async fn undocumented_mounts_stay_out_of_the_spec() {
    let spec = spec(app_with(Mount::new(routes(Trace::default()))).await).await;
    assert!(spec["paths"].as_object().is_none_or(|paths| paths.is_empty()), "{spec}");
}
