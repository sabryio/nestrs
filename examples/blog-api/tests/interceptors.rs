//! The catalogue interceptors, attached to the real controllers.

use blog_api::AppModule;
use nestrs::http::axum::http::StatusCode;
use nestrs_testing::{TestApp, TestClient};
use serde_json::{Value, json};

const TOKEN: &str = "dev-token";

async fn client() -> TestClient {
    TestApp::start(AppModule).await.client()
}

#[tokio::test]
async fn retrying_a_post_with_the_same_key_creates_one_user() {
    let client = client().await;
    let body = json!({ "name": "Ada", "email": "ada@example.com" });
    let send = || async {
        client
            .post("/users")
            .bearer(TOKEN)
            .header("idempotency-key", "signup-1")
            .json(&body)
            .send()
            .await
    };

    let first = send().await.assert_status(StatusCode::CREATED);
    let second = send().await.assert_status(StatusCode::CREATED);
    assert_eq!(second.header("idempotent-replayed"), Some("true"));
    assert_eq!(
        first.json::<Value>()["id"],
        second.json::<Value>()["id"],
        "same user, not a duplicate"
    );

    // Without replay protection the second POST would have been a 409 for the unique email.
    let users: Value = client.get("/users").send().await.json();
    assert_eq!(users.as_array().map(Vec::len), Some(1));
}

#[tokio::test]
async fn the_same_key_for_a_different_body_is_refused() {
    let client = client().await;
    let post = |name: &'static str, email: &'static str| {
        client
            .post("/users")
            .bearer(TOKEN)
            .header("idempotency-key", "k")
            .json(&json!({ "name": name, "email": email }))
            .send()
    };
    post("Ada", "ada@example.com").await.assert_status(StatusCode::CREATED);
    let clash = post("Grace", "grace@example.com")
        .await
        .assert_status(StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(clash.json::<Value>()["error"]["code"], "idempotency_key_reuse");
}

#[tokio::test]
async fn reads_carry_an_etag_and_answer_conditional_requests() {
    let client = client().await;
    let first = client.get("/posts").send().await.assert_status(StatusCode::OK);
    let tag = first.header("etag").expect("an ETag").to_owned();

    let again = client
        .get("/posts")
        .header("if-none-match", &tag)
        .send()
        .await
        .assert_status(StatusCode::NOT_MODIFIED);
    assert_eq!(again.text(), "");
}

#[tokio::test]
async fn responses_are_hardened() {
    let response = client().await.get("/posts").send().await;
    assert_eq!(response.header("x-content-type-options"), Some("nosniff"));
    assert_eq!(response.header("x-frame-options"), Some("DENY"));
    assert_eq!(
        response.header("strict-transport-security"),
        None,
        "HSTS is off for plain HTTP"
    );
}

#[tokio::test]
async fn main_can_resolve_the_shared_rate_limit_store_and_readiness_omits_unconfigured_redis() {
    let app = TestApp::start(AppModule).await;
    // `main` builds the app-wide limit from this port; without Redis it is the in-memory store.
    app.container()
        .resolve::<dyn nestrs::http::RateLimitStore>()
        .expect("the rate limit store is visible from the root");

    let ready: Value = app
        .client()
        .get("/health/ready")
        .send()
        .await
        .assert_status(StatusCode::OK)
        .json();
    let redis = ready["checks"]
        .as_array()
        .and_then(|checks| checks.iter().find(|check| check["name"] == "redis"))
        .cloned();
    assert!(
        redis.is_none(),
        "Redis is not configured, so it is not reported: {ready}"
    );
}
