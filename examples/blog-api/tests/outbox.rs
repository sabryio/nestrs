//! Database mode announces published posts through the outbox: recorded with the post,
//! delivered by the relay to the queue consumer.

use std::time::{Duration, Instant};

use blog_api::AppModule;
use blog_api::infrastructure::announcements::Announced;
use blog_api::infrastructure::config::{AppConfig, Storage};
use nestrs::http::axum::http::StatusCode;
use nestrs_outbox::OutboxRelay;
use nestrs_testing::TestApp;
use serde_json::{Value, json};

async fn database_app() -> TestApp {
    let config = AppConfig {
        storage: Storage::Database,
        database_url: "sqlite::memory:".to_owned(),
        ..AppConfig::default()
    };
    TestApp::builder(AppModule).override_value(config).start().await
}

async fn until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[tokio::test]
async fn publishing_a_post_announces_it_once() {
    let app = database_app().await;
    let client = app.client();
    let announced = app.resolve::<Announced>();

    let author: Value = client
        .post("/users")
        .bearer("dev-token")
        .json(&json!({ "name": "Ada", "email": "ada@example.com" }))
        .send()
        .await
        .assert_status(StatusCode::CREATED)
        .json();
    let post: Value = client
        .post("/posts")
        .bearer("dev-token")
        .json(&json!({ "author_id": author["id"], "title": "Hello", "body": "World" }))
        .send()
        .await
        .assert_status(StatusCode::CREATED)
        .json();

    until("the announcement", || announced.all().len() == 1).await;
    let announcement = announced.all().remove(0);
    assert_eq!(announcement.post_id.to_string(), post["id"].as_str().unwrap());
    assert_eq!(announcement.title, "Hello");

    let relay = app.resolve::<OutboxRelay>();
    until("the relay to finish", || relay.stats().published() == 1).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(announced.all().len(), 1, "delivered once, not again");
}

#[tokio::test]
async fn a_post_that_is_refused_is_not_announced() {
    let app = database_app().await;
    let announced = app.resolve::<Announced>();

    // The author does not exist, so nothing is stored and nothing may be announced.
    app.client()
        .post("/posts")
        .bearer("dev-token")
        .json(&json!({ "author_id": "6c1f4a2e-3f3d-4c43-9d44-7a7c5a2f0001", "title": "Ghost", "body": "Post" }))
        .send()
        .await
        .assert_status(StatusCode::BAD_REQUEST);

    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(announced.all().is_empty());
}
