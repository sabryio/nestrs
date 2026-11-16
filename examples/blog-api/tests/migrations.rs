//! Schema migrations (`sea-orm-migration`): applied once, recorded, reversible,
//! adoptable by databases that predate them, and switchable off.

use blog_api::AppModule;
use blog_api::infrastructure::config::{AppConfig, Storage};
use nestrs::http::axum::http::StatusCode;
use nestrs_sea_orm::Database;
use nestrs_testing::TestApp;
use sea_orm::ConnectionTrait;
use serde_json::json;

const ALL: [&str; 3] = [
    "m20261110_000001_create_users",
    "m20261110_000002_create_posts",
    "m20261115_000001_create_nestrs_outbox",
];

/// A SQLite file that is removed when the test ends.
struct TempDb(std::path::PathBuf);

impl TempDb {
    fn new(label: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        Self(std::env::temp_dir().join(format!("blog-api-{label}-{}-{nanos}.db", std::process::id())))
    }

    fn url(&self) -> String {
        format!("sqlite://{}?mode=rwc", self.0.display())
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn config(url: &str) -> AppConfig {
    AppConfig {
        storage: Storage::Database,
        database_url: url.to_owned(),
        ..AppConfig::default()
    }
}

async fn start(config: AppConfig) -> TestApp {
    TestApp::builder(AppModule).override_value(config).start().await
}

async fn register(app: &TestApp, email: &str) -> StatusCode {
    app.client()
        .post("/users")
        .bearer("dev-token")
        .json(&json!({ "name": "Ada", "email": email }))
        .send()
        .await
        .status()
}

#[tokio::test]
async fn pending_migrations_are_applied_and_recorded_at_startup() {
    let app = start(config("sqlite::memory:")).await;
    let applied = app.resolve::<Database>().applied_migrations().await.unwrap();
    assert_eq!(applied, ALL);
}

#[tokio::test]
async fn restarting_on_the_same_database_applies_nothing_twice_and_keeps_the_data() {
    let file = TempDb::new("restart");

    let first = start(config(&file.url())).await;
    assert_eq!(register(&first, "ada@example.com").await, StatusCode::CREATED);
    first.shutdown().await;

    let second = start(config(&file.url())).await;
    assert_eq!(second.resolve::<Database>().applied_migrations().await.unwrap(), ALL);
    // The row from the first run is still there: the second start did not recreate tables.
    assert_eq!(register(&second, "ada@example.com").await, StatusCode::CONFLICT);
}

#[tokio::test]
async fn migrations_can_be_rolled_back_and_applied_again() {
    let app = start(config("sqlite::memory:")).await;
    let database = app.resolve::<Database>();

    database.roll_back(1).await.unwrap();
    assert_eq!(database.applied_migrations().await.unwrap(), &ALL[..2]);

    database.roll_back(2).await.unwrap();
    assert!(database.applied_migrations().await.unwrap().is_empty());

    database.migrate().await.unwrap();
    assert_eq!(database.applied_migrations().await.unwrap(), ALL);
    assert_eq!(register(&app, "after@example.com").await, StatusCode::CREATED);
}

#[tokio::test]
async fn a_database_that_predates_migrations_adopts_them_without_losing_data() {
    let file = TempDb::new("legacy");
    {
        // What the application created before it had migrations (SeaORM stores UUIDs
        // as 16-byte blobs in SQLite).
        let legacy = sea_orm::Database::connect(file.url()).await.unwrap();
        legacy
            .execute_unprepared(
                "CREATE TABLE users (id uuid NOT NULL PRIMARY KEY, name varchar NOT NULL, email varchar NOT NULL UNIQUE);\
                 CREATE TABLE posts (id uuid NOT NULL PRIMARY KEY, author_id uuid NOT NULL, title varchar NOT NULL, body text NOT NULL);\
                 INSERT INTO users VALUES (X'11111111111111111111111111111111', 'Legacy', 'legacy@example.com');",
            )
            .await
            .unwrap();
        legacy.close().await.unwrap();
    }

    let app = start(config(&file.url())).await;
    assert_eq!(app.resolve::<Database>().applied_migrations().await.unwrap(), ALL);
    let users: serde_json::Value = app.client().get("/users").send().await.json();
    assert_eq!(users[0]["email"], "legacy@example.com", "{users}");
}

#[tokio::test]
async fn migrating_can_be_left_to_a_separate_deployment_step() {
    let mut settings = config("sqlite::memory:");
    settings.migrate_on_start = false;
    let app = start(settings).await;

    // Nothing was applied, so the tables do not exist yet.
    assert!(app.client().get("/users").send().await.status().is_server_error());

    app.resolve::<Database>().migrate().await.unwrap();
    assert_eq!(app.client().get("/users").send().await.status(), StatusCode::OK);
}

/// Runs against a real Postgres when `BLOG_TEST_POSTGRES_URL` is set, e.g.
/// `postgres://postgres:postgres@localhost:5432/blog_test`. The database is reset first.
#[tokio::test]
async fn the_same_migrations_run_on_postgres() {
    let Ok(url) = std::env::var("BLOG_TEST_POSTGRES_URL") else {
        eprintln!("skipped: BLOG_TEST_POSTGRES_URL is not set");
        return;
    };
    {
        let connection = sea_orm::Database::connect(&url).await.unwrap();
        connection
            .execute_unprepared("DROP TABLE IF EXISTS posts, users, seaql_migrations CASCADE")
            .await
            .unwrap();
        connection.close().await.unwrap();
    }

    let app = start(config(&url)).await;
    assert_eq!(app.resolve::<Database>().applied_migrations().await.unwrap(), ALL);
    let author: serde_json::Value = app
        .client()
        .post("/users")
        .bearer("dev-token")
        .json(&json!({ "name": "Ada", "email": "pg@example.com" }))
        .send()
        .await
        .assert_status(StatusCode::CREATED)
        .json();
    assert_eq!(register(&app, "PG@example.com").await, StatusCode::CONFLICT);
    // A post exercises the foreign key to users.
    app.client()
        .post("/posts")
        .bearer("dev-token")
        .json(&json!({ "author_id": author["id"], "title": "On Postgres", "body": "It works" }))
        .send()
        .await
        .assert_status(StatusCode::CREATED);

    // Reverting drops the tables (posts first, because of the foreign key) and
    // migrating again recreates them.
    app.resolve::<Database>().roll_back(2).await.unwrap();
    app.resolve::<Database>().migrate().await.unwrap();
    assert_eq!(app.resolve::<Database>().applied_migrations().await.unwrap(), ALL);
    app.shutdown().await;
}
