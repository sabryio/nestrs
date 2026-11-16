//! The point of the crate, end to end: an order and its event commit together or not at all,
//! and a committed event reaches its queue handler exactly once.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use nestrs::http::axum::http::StatusCode;
use nestrs::prelude::*;
use nestrs_jobs::{HandlerError, JobsModule, QueueHandler, QueueHandlerPort, decode};
use nestrs_outbox::{
    CreateOutbox, Outbox, OutboxConfig, OutboxEnvelope, OutboxModule, OutboxPublisherPort, OutboxRelay,
    OutboxStorePort, QueuePublisher, SeaOutboxStore,
};
use nestrs_sea_orm::sea_orm::{ConnectionTrait, DbBackend, DbErr, Statement};
use nestrs_sea_orm::sea_orm_migration::prelude::{
    MigrationName, MigrationTrait, MigratorTrait, SchemaManager, async_trait,
};
use nestrs_sea_orm::{Database, DatabaseConfig, SeaOrmModule, Transactional};
use nestrs_testing::TestApp;

struct CreateOrders;

impl MigrationName for CreateOrders {
    fn name(&self) -> &str {
        "m20261115_000000_create_orders"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for CreateOrders {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("CREATE TABLE orders (id TEXT PRIMARY KEY)")
            .await
            .map(|_| ())
    }
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE orders")
            .await
            .map(|_| ())
    }
}

struct Migrator;

impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![Box::new(CreateOrders), Box::new(CreateOutbox)]
    }
}

/// Database, transactions and the outbox table's store.
#[module(
    imports = [SeaOrmModule::with_migrator::<Migrator>(Provider::value(DatabaseConfig::new("sqlite::memory:")))],
    providers = [SeaOutboxStore],
    exports = [Database, Transactional, OutboxStorePort],
)]
struct Persistence;

/// An in-process queue and the publisher that feeds it.
#[module(
    imports = [JobsModule::in_memory()],
    providers = [QueuePublisher],
    exports = [OutboxPublisherPort],
)]
struct Messaging;

#[injectable]
struct Orders {
    database: Arc<Database>,
}

fn internal(error: impl std::fmt::Display) -> HttpError {
    HttpError::internal(error.to_string())
}

impl Orders {
    /// Writes the order and records its event, both through the request's executor.
    async fn place(&self, id: &str) -> Result<(), HttpError> {
        let db = self.database.executor().await.map_err(internal)?;
        db.execute_unprepared(&format!("INSERT INTO orders (id) VALUES ('{id}')"))
            .await
            .map_err(internal)?;
        Outbox
            .record_json(&db, "orders.placed", Some(id), &serde_json::json!({ "order": id }))
            .await
            .map_err(internal)?;
        Ok(())
    }
}

#[controller("/orders")]
impl Orders {
    #[post("/ok")]
    #[interceptor(Transactional)]
    #[status(201)]
    async fn ok(&self) -> Result<(), HttpError> {
        self.place("o-1").await
    }

    /// Records the same things, then fails: nothing may survive.
    #[post("/fail")]
    #[interceptor(Transactional)]
    async fn fail(&self) -> Result<(), HttpError> {
        self.place("o-2").await?;
        Err(HttpError::bad_request("changed my mind"))
    }

    #[get("/count")]
    async fn count(&self) -> Result<Json<i64>, HttpError> {
        let db = self.database.executor().await.map_err(internal)?;
        let row = db
            .query_one(Statement::from_string(
                DbBackend::Sqlite,
                "SELECT COUNT(*) AS n FROM orders",
            ))
            .await
            .map_err(internal)?
            .ok_or_else(|| internal("no row"))?;
        Ok(Json(row.try_get("", "n").map_err(internal)?))
    }
}

/// What the consumer was handed; the tests read it.
#[injectable]
struct Inbox {
    #[inject(default)]
    received: Mutex<Vec<OutboxEnvelope<serde_json::Value>>>,
}

/// The consumer.
#[injectable(contributes = QueueHandlerPort)]
struct OrderPlaced {
    inbox: Arc<Inbox>,
}

#[async_trait]
impl QueueHandler for OrderPlaced {
    fn queue(&self) -> &str {
        "orders.placed"
    }
    async fn handle(&self, payload: &str) -> Result<(), HandlerError> {
        let envelope = decode(payload)?;
        self.inbox.received.lock().unwrap().push(envelope);
        Ok(())
    }
}

#[module(
    imports = [
        Persistence,
        Messaging,
        OutboxModule::for_root(
            OutboxConfig { poll_interval: Duration::from_millis(20), ..OutboxConfig::default() },
            [ModuleRef::of::<Persistence>(), ModuleRef::of::<Messaging>()],
        ),
    ],
    providers = [Inbox, OrderPlaced],
    controllers = [Orders],
)]
struct AppModule;

async fn until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn a_committed_order_is_announced_exactly_once() {
    let app = TestApp::start(AppModule).await;
    let consumer = app.resolve::<Inbox>();

    app.client()
        .post("/orders/ok")
        .send()
        .await
        .assert_status(StatusCode::CREATED);

    until("the event to arrive", || consumer.received.lock().unwrap().len() == 1).await;
    {
        let received = consumer.received.lock().unwrap();
        assert_eq!(received[0].topic, "orders.placed");
        assert_eq!(received[0].payload["order"], "o-1");
        assert!(
            !received[0].id.is_empty(),
            "the message id travels with it, for deduplication"
        );
    }
    // Delivered, so removed: waiting longer does not produce a second copy.
    let relay = app.resolve::<OutboxRelay>();
    until("the relay to forget it", || relay.stats().published() == 1).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(consumer.received.lock().unwrap().len(), 1);
    assert_eq!(relay.stats().published(), 1);
}

#[tokio::test]
async fn a_rolled_back_order_announces_nothing() {
    let app = TestApp::start(AppModule).await;
    let consumer = app.resolve::<Inbox>();
    let relay = app.resolve::<OutboxRelay>();

    app.client()
        .post("/orders/fail")
        .send()
        .await
        .assert_status(StatusCode::BAD_REQUEST);

    tokio::time::sleep(Duration::from_millis(300)).await;
    let orders: i64 = app.client().get("/orders/count").send().await.json();
    assert_eq!(orders, 0, "the order was rolled back");
    assert!(consumer.received.lock().unwrap().is_empty(), "and so was its event");
    assert_eq!(relay.stats().published(), 0);
}
