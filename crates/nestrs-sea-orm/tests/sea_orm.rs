//! The crate through its public surface: module wiring, migrations, health, explicit and
//! per-request transactions, on SQLite in memory.

use nestrs::http::axum::http::StatusCode;
use nestrs::prelude::*;
use nestrs_health::HealthModule;
use nestrs_sea_orm::sea_orm::{ConnectionTrait, DbBackend, DbErr, Statement};
use nestrs_sea_orm::sea_orm_migration::prelude::{
    DeriveMigrationName, MigrationTrait, MigratorTrait, SchemaManager, async_trait,
};
use nestrs_sea_orm::*;
use nestrs_testing::TestApp;

// -- a tiny schema ----------------------------------------------------------------------

#[derive(DeriveMigrationName)]
struct CreateNotes;

#[async_trait::async_trait]
impl MigrationTrait for CreateNotes {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("CREATE TABLE notes (id INTEGER PRIMARY KEY AUTOINCREMENT, body TEXT NOT NULL)")
            .await
            .map(|_| ())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE notes")
            .await
            .map(|_| ())
    }
}

struct Migrator;

impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![Box::new(CreateNotes)]
    }
}

async fn insert(db: &Executor, body: &str) -> Result<(), DbError> {
    db.execute(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "INSERT INTO notes (body) VALUES (?)",
        [body.into()],
    ))
    .await?;
    Ok(())
}

async fn count(db: &Executor) -> i64 {
    let row = db
        .query_one(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT COUNT(*) AS n FROM notes",
        ))
        .await
        .unwrap()
        .unwrap();
    row.try_get("", "n").unwrap()
}

// -- an application using the module -----------------------------------------------------

#[injectable]
struct Notes {
    database: Arc<Database>,
}

#[controller("/notes")]
impl Notes {
    /// Writes two rows and succeeds: both are kept.
    #[post("/ok")]
    #[interceptor(Transactional)]
    #[status(201)]
    async fn ok(&self) -> Result<(), HttpError> {
        let db = self
            .database
            .executor()
            .await
            .map_err(|e| HttpError::internal(e.to_string()))?;
        assert!(db.in_transaction());
        for body in ["a", "b"] {
            insert(&db, body)
                .await
                .map_err(|e| HttpError::internal(e.to_string()))?;
        }
        Ok(())
    }

    /// Writes a row, then fails: the row must not survive.
    #[post("/fail")]
    #[interceptor(Transactional)]
    async fn fail(&self) -> Result<(), HttpError> {
        let db = self
            .database
            .executor()
            .await
            .map_err(|e| HttpError::internal(e.to_string()))?;
        insert(&db, "lost")
            .await
            .map_err(|e| HttpError::internal(e.to_string()))?;
        Err(HttpError::bad_request("no"))
    }

    /// Without the interceptor a request talks to the pool.
    #[get("/count")]
    async fn count(&self) -> Result<Json<i64>, HttpError> {
        let db = self
            .database
            .executor()
            .await
            .map_err(|e| HttpError::internal(e.to_string()))?;
        assert!(!db.in_transaction());
        Ok(Json(count(&db).await))
    }
}

#[module(
    imports = [
        HealthModule,
        SeaOrmModule::with_migrator::<Migrator>(Provider::value(DatabaseConfig::new("sqlite::memory:"))),
    ],
    controllers = [Notes],
)]
struct AppModule;

async fn app() -> TestApp {
    TestApp::start(AppModule).await
}

// -- tests -------------------------------------------------------------------------------

#[tokio::test]
async fn migrations_run_on_start_and_can_be_rolled_back() {
    let app = app().await;
    let database = app.resolve::<Database>();

    assert_eq!(database.applied_migrations().await.unwrap().len(), 1);
    // The table exists, so the first request already works.
    app.client()
        .get("/notes/count")
        .send()
        .await
        .assert_status(StatusCode::OK);

    database.roll_back(1).await.unwrap();
    assert!(database.applied_migrations().await.unwrap().is_empty());
    database.migrate().await.unwrap();
    assert_eq!(database.applied_migrations().await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_successful_request_commits_and_a_failed_one_rolls_back() {
    let app = app().await;
    let client = app.client();

    client.post("/notes/ok").send().await.assert_status(StatusCode::CREATED);
    client
        .post("/notes/fail")
        .send()
        .await
        .assert_status(StatusCode::BAD_REQUEST);

    let count: i64 = client.get("/notes/count").send().await.json();
    assert_eq!(count, 2, "the two rows of /ok stay, the row of /fail is gone");
}

#[tokio::test]
async fn explicit_transactions_commit_on_ok_and_roll_back_on_err() {
    let app = app().await;
    let database = app.resolve::<Database>();

    let committed: Result<(), DbError> = database
        .in_transaction(|tx| {
            Box::pin(async move {
                assert!(tx.in_transaction());
                insert(tx, "kept").await
            })
        })
        .await;
    committed.unwrap();

    let rolled_back: Result<(), DbError> = database
        .in_transaction(|tx| {
            Box::pin(async move {
                insert(tx, "dropped").await?;
                Err(DbError::Query("changed my mind".to_owned()))
            })
        })
        .await;
    assert!(rolled_back.is_err());

    let pool = database.executor().await.unwrap();
    assert_eq!(count(&pool).await, 1);
}

#[tokio::test]
async fn a_unit_of_work_rolls_back_when_dropped() {
    let app = app().await;
    let database = app.resolve::<Database>();

    let unit = database.begin().await.unwrap();
    insert(&unit.executor(), "abandoned").await.unwrap();
    drop(unit);

    let unit = database.begin().await.unwrap();
    insert(&unit.executor(), "committed").await.unwrap();
    unit.commit().await.unwrap();

    let pool = database.executor().await.unwrap();
    assert_eq!(count(&pool).await, 1);
}

#[tokio::test]
async fn committing_while_an_executor_is_alive_is_an_error_not_a_hang() {
    let app = app().await;
    let database = app.resolve::<Database>();

    let unit = database.begin().await.unwrap();
    let leaked = unit.executor();
    let error = unit.commit().await.unwrap_err();
    assert!(matches!(error, DbError::Transaction(_)), "{error}");
    drop(leaked);
}

#[tokio::test]
async fn the_database_reports_readiness_through_the_health_port() {
    let app = app().await;
    let response = app
        .client()
        .get("/health/ready")
        .send()
        .await
        .assert_status(StatusCode::OK);
    let report: serde_json::Value = response.json();
    assert_eq!(report["checks"][0]["name"], "database", "{report}");
}

#[tokio::test]
async fn a_disconnected_database_is_inert_and_says_so() {
    let database = Database::disconnected();
    assert!(!database.is_connected());
    database.ping().await.unwrap();
    database.migrate().await.unwrap();
    assert_eq!(database.executor().await.unwrap_err(), DbError::NotConfigured);
    assert_eq!(database.begin().await.err(), Some(DbError::NotConfigured));
}

#[tokio::test]
async fn disabled_in_config_means_disconnected() {
    let config = DatabaseConfig::new("postgres://nowhere.invalid/x").enabled(false);
    let database = Database::connect(&config, std::sync::Arc::new(NoMigrations))
        .await
        .unwrap();
    assert!(!database.is_connected());
}

#[tokio::test]
async fn an_unreachable_database_fails_connect_with_a_clear_error() {
    let config = DatabaseConfig::new("sqlite:///nonexistent-dir/db.sqlite?mode=ro")
        .connect_timeout(std::time::Duration::from_secs(1));
    let error = Database::connect(&config, std::sync::Arc::new(NoMigrations))
        .await
        .err()
        .expect("must fail");
    assert!(matches!(error, DbError::Connect(_)), "{error}");
}

#[test]
#[should_panic(expected = "needs a provider of `DatabaseConfig`")]
fn the_module_rejects_a_provider_of_the_wrong_type() {
    let _ = SeaOrmModule::without_migrations(Provider::value(42_u32));
}
