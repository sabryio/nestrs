//! End-to-end tests for the macros: declare modules and controllers the way an
//! application would, bootstrap them, and drive the resulting router.

use std::sync::atomic::{AtomicUsize, Ordering};

use http_body_util::BodyExt;
use nestrs::http::axum::Router;
use nestrs::http::axum::body::Body;
use nestrs::http::axum::http::{Method, Request};
use nestrs::prelude::*;
use serde_json::{Value, json};
use tower::ServiceExt;

// ------------------------------------------------------------------ providers

trait Greeter: Send + Sync {
    fn greet(&self, name: &str) -> String;
}

#[injectable]
struct PoliteGreeter;

impl Greeter for PoliteGreeter {
    fn greet(&self, name: &str) -> String {
        format!("Good day, {name}")
    }
}

#[injectable]
struct Counter {
    #[inject(default)]
    hits: AtomicUsize,
}

// --------------------------------------------------------------------- guards

#[injectable]
struct KeyGuard;

impl Guard for KeyGuard {
    async fn check(&self, parts: &mut Parts) -> Result<(), HttpError> {
        match parts.headers.get("x-key") {
            Some(key) if key == "open" => Ok(()),
            _ => Err(HttpError::unauthorized("missing key")),
        }
    }
}

#[injectable]
struct DenyAll;

impl Guard for DenyAll {
    async fn check(&self, _parts: &mut Parts) -> Result<(), HttpError> {
        Err(HttpError::forbidden("closed"))
    }
}

// ---------------------------------------------------------------- controllers

#[injectable]
struct GreetController {
    greeter: Arc<dyn Greeter>,
    counter: Arc<Counter>,
}

#[controller("/greet")]
impl GreetController {
    #[get("/{name}")]
    async fn hello(&self, Path(name): Path<String>) -> String {
        self.counter.hits.fetch_add(1, Ordering::SeqCst);
        self.greeter.greet(&name)
    }

    #[get]
    async fn stats(&self) -> Json<Value> {
        Json(json!({ "hits": self.counter.hits.load(Ordering::SeqCst) }))
    }

    #[post("/secret")]
    #[guard(KeyGuard)]
    async fn open_secret(&self) -> StatusCode {
        StatusCode::CREATED
    }

    #[delete("/secret")]
    #[guard(DenyAll)]
    async fn closed_secret(&self) -> StatusCode {
        StatusCode::NO_CONTENT
    }
}

/// Class-level guards apply to every route, and run before method-level ones.
#[injectable]
struct VaultController;

#[controller("vault")]
#[guard(KeyGuard)]
impl VaultController {
    #[get("/")]
    async fn peek(&self) -> &'static str {
        "gold"
    }

    #[put("/")]
    #[guard(DenyAll)]
    async fn replace(&self) -> &'static str {
        "unreachable"
    }
}

#[module(
    providers = [bind!(dyn Greeter => PoliteGreeter), Counter, KeyGuard, DenyAll],
    controllers = [GreetController, VaultController],
)]
struct AppModule;

// -------------------------------------------------------------------- helpers

async fn router() -> Router {
    App::create(AppModule)
        .await
        .expect("module graph is valid")
        .into_router()
}

async fn send(router: &Router, method: Method, uri: &str, headers: &[(&str, &str)]) -> (StatusCode, String) {
    let mut request = Request::builder().method(method).uri(uri);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = router
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8(body.to_vec()).unwrap())
}

// ---------------------------------------------------------------------- tests

#[tokio::test]
async fn routes_resolve_dependencies_and_extractors() {
    let router = router().await;

    let (status, body) = send(&router, Method::GET, "/greet/Ana", &[]).await;
    assert_eq!((status, body.as_str()), (StatusCode::OK, "Good day, Ana"));

    // `#[get]` with no path maps to the bare prefix, and shares the same singleton.
    let (status, body) = send(&router, Method::GET, "/greet", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(serde_json::from_str::<Value>(&body).unwrap(), json!({ "hits": 1 }));
}

#[tokio::test]
async fn method_guards_protect_only_their_route() {
    let router = router().await;

    let (status, body) = send(&router, Method::POST, "/greet/secret", &[]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let body: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(body["error"]["code"], "unauthorized");

    let (status, _) = send(&router, Method::POST, "/greet/secret", &[("x-key", "open")]).await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, _) = send(&router, Method::DELETE, "/greet/secret", &[("x-key", "open")]).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn class_guards_run_before_method_guards() {
    let router = router().await;

    // KeyGuard (class-level) rejects first, so DenyAll's 403 is never reached.
    let (status, _) = send(&router, Method::PUT, "/vault", &[]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _) = send(&router, Method::PUT, "/vault", &[("x-key", "open")]).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, body) = send(&router, Method::GET, "/vault", &[("x-key", "open")]).await;
    assert_eq!((status, body.as_str()), (StatusCode::OK, "gold"));
}

#[tokio::test]
async fn unknown_routes_use_the_json_envelope() {
    let router = router().await;
    let (status, body) = send(&router, Method::GET, "/nope", &[]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap()["error"]["code"],
        "not_found"
    );
}

#[tokio::test]
async fn global_prefix_nests_everything() {
    let router = App::create(AppModule)
        .await
        .unwrap()
        .global_prefix("/api")
        .into_router();

    let (status, _) = send(&router, Method::GET, "/api/greet/Ana", &[]).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(&router, Method::GET, "/greet/Ana", &[]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ------------------------------------------------------------- wiring errors

#[injectable]
struct NeedsGreeter {
    #[allow(dead_code)]
    greeter: Arc<dyn Greeter>,
}

#[module(providers = [NeedsGreeter])]
struct Broken;

#[tokio::test]
async fn missing_dependencies_fail_at_startup_with_a_readable_message() {
    let error = App::create(Broken).await.unwrap_err();
    assert!(matches!(
        error,
        AppError::Di(DiError::MissingProvider { module: "Broken", .. })
    ));
    let message = error.to_string();
    assert!(
        message.contains("NeedsGreeter") && message.contains("dyn Greeter"),
        "{message}"
    );
}

#[injectable]
struct Locked;

#[controller("/locked")]
impl Locked {
    #[get]
    #[guard(KeyGuard)]
    async fn get(&self) -> &'static str {
        "x"
    }
}

#[module(providers = [KeyGuard])]
struct PrivateAuth;

#[module(imports = [PrivateAuth], controllers = [Locked])]
struct WebWithPrivateGuard;

#[tokio::test]
async fn guards_are_checked_for_visibility_like_any_dependency() {
    let error = App::create(WebWithPrivateGuard).await.unwrap_err();
    assert!(
        matches!(
            error,
            AppError::Di(DiError::NotVisible {
                module: "WebWithPrivateGuard",
                provided_in: "PrivateAuth",
                ..
            })
        ),
        "{error}"
    );
}

#[module(providers = [KeyGuard], exports = [KeyGuard])]
struct PublicAuth;

#[module(imports = [PublicAuth], controllers = [Locked])]
struct WebWithPublicGuard;

#[tokio::test]
async fn exporting_the_guard_fixes_it() {
    let router = App::create(WebWithPublicGuard).await.unwrap().into_router();
    let (status, _) = send(&router, Method::GET, "/locked", &[]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = send(&router, Method::GET, "/locked", &[("x-key", "open")]).await;
    assert_eq!(status, StatusCode::OK);
}
