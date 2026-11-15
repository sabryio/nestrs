//! The harness against a small real application.

use std::sync::Arc;

use nestrs::prelude::*;
use nestrs_testing::TestApp;
use serde_json::{Value, json};

trait Greeter: Send + Sync {
    fn greet(&self, name: &str) -> String;
}
#[injectable]
struct English;
impl Greeter for English {
    fn greet(&self, name: &str) -> String {
        format!("Hello, {name}")
    }
}
struct Pirate;
impl Greeter for Pirate {
    fn greet(&self, name: &str) -> String {
        format!("Ahoy, {name}")
    }
}

#[injectable]
struct Welcome {
    greeter: Arc<dyn Greeter>,
}

#[injectable]
struct Gate;
impl Guard for Gate {
    async fn check(&self, parts: &mut Parts) -> Result<(), HttpError> {
        match parts.headers.get("authorization").and_then(|v| v.to_str().ok()) {
            Some("Bearer ok") => Ok(()),
            _ => Err(HttpError::unauthorized("nope")),
        }
    }
}

#[injectable]
struct WelcomeController {
    welcome: Arc<Welcome>,
}

#[controller("/welcome")]
impl WelcomeController {
    #[get("/{name}")]
    async fn show(&self, Path(name): Path<String>) -> Json<Value> {
        Json(json!({ "message": self.welcome.greeter.greet(&name) }))
    }

    #[post]
    #[guard(Gate)]
    async fn echo(&self, Json(body): Json<Value>) -> (StatusCode, Json<Value>) {
        (StatusCode::CREATED, Json(body))
    }
}

#[module(
    providers = [bind!(dyn Greeter => English), Welcome, Gate],
    controllers = [WelcomeController],
)]
struct AppModule;

#[tokio::test]
async fn starts_the_real_module_and_talks_to_it() {
    let app = TestApp::start(AppModule).await;
    let body: Value = app
        .client()
        .get("/welcome/Ada")
        .send()
        .await
        .assert_status(StatusCode::OK)
        .json();
    assert_eq!(body["message"], "Hello, Ada");
}

#[tokio::test]
async fn overrides_swap_an_interface_for_a_fake() {
    let app = TestApp::builder(AppModule)
        .override_instance::<dyn Greeter>(Arc::new(Pirate))
        .start()
        .await;
    let body: Value = app.client().get("/welcome/Ada").send().await.json();
    assert_eq!(body["message"], "Ahoy, Ada");
}

#[tokio::test]
async fn the_client_covers_headers_bearer_json_and_raw_bodies() {
    let app = TestApp::start(AppModule).await;
    let client = app.client();

    client
        .post("/welcome")
        .json(&json!({}))
        .send()
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    let created = client
        .post("/welcome")
        .bearer("ok")
        .json(&json!({ "a": 1 }))
        .send()
        .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    assert_eq!(created.json::<Value>(), json!({ "a": 1 }));

    let malformed = client
        .post("/welcome")
        .bearer("ok")
        .header("content-type", "application/json")
        .body("{not json")
        .send()
        .await;
    assert!(malformed.status().is_client_error());
    assert!(client.get("/nowhere").send().await.text().contains("not_found"));
}

#[tokio::test]
async fn startup_failures_can_be_asserted_instead_of_panicking() {
    let error = TestApp::builder(AppModule)
        .override_value("nobody provides a String".to_owned())
        .try_start()
        .await
        .err()
        .expect("an unknown override must fail startup");
    assert!(error.to_string().contains("override"), "{error}");
}

#[tokio::test]
async fn resolve_exposes_providers_and_prefixes_apply() {
    let app = TestApp::builder(AppModule).global_prefix("/api").start().await;
    let _welcome: Arc<Welcome> = app.resolve();
    app.client()
        .get("/api/welcome/Ada")
        .send()
        .await
        .assert_status(StatusCode::OK);
    app.client()
        .get("/welcome/Ada")
        .send()
        .await
        .assert_status(StatusCode::NOT_FOUND);
    assert!(app.shutdown().await.is_empty());
}
