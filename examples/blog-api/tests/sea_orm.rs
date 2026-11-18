//! The same application on a real SQL database (SQLite in memory through SeaORM),
//! selected purely by configuration: no module, use case or controller changes.

use blog_api::AppModule;
use blog_api::infrastructure::config::{AppConfig, Storage};
use purlin::http::axum::http::StatusCode;
use purlin_testing::{TestApp, TestClient};
use serde_json::{Value, json};

fn database_config() -> AppConfig {
    AppConfig {
        storage: Storage::Database,
        database_url: "sqlite::memory:".to_owned(),
        ..AppConfig::default()
    }
}

async fn app_with(config: AppConfig) -> TestApp {
    TestApp::builder(AppModule).override_value(config).start().await
}

async fn create_user(client: &TestClient, name: &str, email: &str) -> (StatusCode, Value) {
    let response = client
        .post("/users")
        .bearer("dev-token")
        .json(&json!({ "name": name, "email": email }))
        .send()
        .await;
    (response.status(), response.json())
}

#[tokio::test]
async fn users_and_posts_are_persisted_in_the_database() {
    let app = app_with(database_config()).await;
    let client = app.client();

    // The init hook created the tables, so the very first request already works.
    let users: Value = client.get("/users").send().await.assert_status(StatusCode::OK).json();
    assert_eq!(users, json!([]));

    let (status, author) = create_user(&client, "Ada", "Ada@Example.com").await;
    assert_eq!(status, StatusCode::CREATED, "{author}");
    assert_eq!(author["email"], "ada@example.com");

    let fetched: Value = client
        .get(&format!("/users/{}", author["id"].as_str().unwrap()))
        .send()
        .await
        .assert_status(StatusCode::OK)
        .json();
    assert_eq!(fetched, author);

    let post: Value = client
        .post("/posts")
        .bearer("dev-token")
        .json(&json!({ "author_id": author["id"], "title": "  Hello  ", "body": "World" }))
        .send()
        .await
        .assert_status(StatusCode::CREATED)
        .json();
    assert_eq!(post["title"], "Hello");

    let uri = format!("/posts/{}", post["id"].as_str().unwrap());
    let fetched: Value = client.get(&uri).send().await.json();
    assert_eq!(fetched, post);
    client
        .delete(&uri)
        .bearer("dev-token")
        .send()
        .await
        .assert_status(StatusCode::NO_CONTENT);
    client.get(&uri).send().await.assert_status(StatusCode::NOT_FOUND);

    assert!(app.shutdown().await.is_empty(), "the pool closes cleanly");
}

#[tokio::test]
async fn pagination_is_applied_by_the_database() {
    let client = app_with(database_config()).await.client();
    for n in 0..5 {
        create_user(&client, &format!("User {n}"), &format!("user{n}@example.com")).await;
    }
    let all: Value = client.get("/users").send().await.json();
    let page: Value = client.get("/users?limit=2&offset=1").send().await.json();
    assert_eq!(page.as_array().unwrap().len(), 2);
    assert_eq!(page[0], all[1]);
    assert_eq!(page[1], all[2]);
}

#[tokio::test]
async fn the_unique_email_rule_holds_on_the_database_path() {
    let client = app_with(database_config()).await.client();
    assert_eq!(
        create_user(&client, "Ada", "ada@example.com").await.0,
        StatusCode::CREATED
    );
    let (status, error) = create_user(&client, "Ada", "ADA@example.com").await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error["error"]["code"], "conflict");
}

#[tokio::test]
async fn the_seeder_runs_after_the_database_is_migrated() {
    let client = app_with(AppConfig {
        seed_demo_data: true,
        ..database_config()
    })
    .await
    .client();
    let users: Value = client.get("/users").send().await.json();
    assert_eq!(users[0]["email"], "demo@example.com");
    let posts: Value = client.get("/posts").send().await.json();
    assert_eq!(posts.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn an_unreachable_database_fails_startup_with_a_clear_error() {
    let error = TestApp::builder(AppModule)
        .override_value(AppConfig {
            database_url: "postgres://nobody:nothing@127.0.0.1:1/none".to_owned(),
            ..database_config()
        })
        .try_start()
        .await
        .err()
        .expect("must fail");
    assert!(error.to_string().contains("cannot connect to the database"), "{error}");
}

#[tokio::test]
async fn memory_mode_creates_no_connection() {
    let client = app_with(AppConfig::default()).await.client();
    client.get("/users").send().await.assert_status(StatusCode::OK);
}
