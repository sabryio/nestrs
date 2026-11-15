//! Swapping providers in the real `AppModule`, and lifecycle hooks, with `nestrs-testing`.

use std::sync::Arc;

use async_trait::async_trait;
use blog_api::AppModule;
use blog_api::domain::{Email, Page, RepositoryError, User, UserName, UserRepository, UserRepositoryPort};
use blog_api::infrastructure::config::AppConfig;
use nestrs::http::axum::http::StatusCode;
use nestrs::prelude::*;
use nestrs_testing::TestApp;
use serde_json::{Value, json};
use uuid::Uuid;

/// A repository that already knows one user, and nothing else.
struct SeededRepository {
    user: User,
}

#[async_trait]
impl UserRepository for SeededRepository {
    async fn find(&self, id: Uuid) -> Result<Option<User>, RepositoryError> {
        Ok((id == self.user.id()).then(|| self.user.clone()))
    }
    async fn find_by_email(&self, email: &Email) -> Result<Option<User>, RepositoryError> {
        Ok((email == self.user.email()).then(|| self.user.clone()))
    }
    async fn list(&self, _page: Page) -> Result<Vec<User>, RepositoryError> {
        Ok(vec![self.user.clone()])
    }
    async fn insert(&self, _user: User) -> Result<(), RepositoryError> {
        Ok(())
    }
}

fn seeded_user() -> User {
    User::register(
        UserName::parse("Seeded").unwrap(),
        Email::parse("seeded@example.com").unwrap(),
    )
}

#[tokio::test]
async fn overriding_the_config_changes_which_token_is_accepted() {
    let app = TestApp::builder(AppModule)
        .override_value(AppConfig {
            api_token: "test-token".to_owned(),
            ..AppConfig::default()
        })
        .start()
        .await;
    let client = app.client();
    let user = json!({ "name": "Ada", "email": "ada@example.com" });

    client
        .post("/users")
        .bearer("dev-token")
        .json(&user)
        .send()
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    client
        .post("/users")
        .bearer("test-token")
        .json(&user)
        .send()
        .await
        .assert_status(StatusCode::CREATED);
}

#[tokio::test]
async fn overriding_a_repository_reaches_every_dependent_across_modules() {
    let user = seeded_user();
    let app = TestApp::builder(AppModule)
        .mock::<UserRepositoryPort>(Arc::new(SeededRepository { user: user.clone() }))
        .start()
        .await;
    let client = app.client();

    // Directly through the users feature...
    let users: Value = client.get("/users").send().await.assert_status(StatusCode::OK).json();
    assert_eq!(users[0]["email"], "seeded@example.com");

    // ...and indirectly: publishing validates authors through the same repository.
    client
        .post("/posts")
        .bearer("dev-token")
        .json(&json!({ "author_id": user.id(), "title": "Hello", "body": "World" }))
        .send()
        .await
        .assert_status(StatusCode::CREATED);
}

#[tokio::test]
async fn overrides_are_validated_like_the_rest_of_the_graph() {
    let error = TestApp::builder(AppModule)
        .override_value("nobody provides a String".to_owned())
        .try_start()
        .await
        .err()
        .expect("must fail");
    assert!(
        matches!(error, AppError::Di(DiError::UnknownOverride { .. })),
        "{error}"
    );
}

#[tokio::test]
async fn the_seeder_runs_as_an_init_hook_and_shutdown_hooks_run_once() {
    let app = TestApp::builder(AppModule)
        .override_value(AppConfig {
            seed_demo_data: true,
            ..AppConfig::default()
        })
        .start()
        .await;
    let client = app.client();

    let users: Value = client.get("/users").send().await.json();
    assert_eq!(users[0]["email"], "demo@example.com", "seeded before the first request");
    let posts: Value = client.get("/posts").send().await.json();
    assert_eq!(posts.as_array().unwrap().len(), 1);

    assert!(app.shutdown().await.is_empty());
    assert!(app.shutdown().await.is_empty(), "second shutdown is a no-op");
}

#[tokio::test]
async fn nothing_is_seeded_by_default() {
    let users: Value = TestApp::start(AppModule)
        .await
        .client()
        .get("/users")
        .send()
        .await
        .json();
    assert_eq!(users, json!([]));
}

#[tokio::test]
async fn invalid_configuration_stops_startup() {
    // The real config module, fed an impossible value through its own source.
    use nestrs::config::{ConfigModule, ConfigSource};

    #[module(imports = [ConfigModule::for_root::<AppConfig>(ConfigSource::new().set("api_token", ""))])]
    struct Misconfigured;

    let error = TestApp::builder(Misconfigured)
        .try_start()
        .await
        .err()
        .expect("must fail");
    assert!(error.to_string().contains("`api_token` must not be empty"), "{error}");
}
