//! One contract, two implementations. Every store must pass every case, which is what makes
//! them interchangeable (and the relay indifferent to which one it drives).

use std::sync::Arc;
use std::time::Duration;

use nestrs::async_trait;
use nestrs_outbox::{
    CreateOutbox, MemoryOutboxStore, NewMessage, Outbox, OutboxMessage, OutboxStorePort, SeaOutboxStore,
};
use nestrs_sea_orm::sea_orm_migration::prelude::{MigrationTrait, MigratorTrait};
use nestrs_sea_orm::{Database, DatabaseConfig, SeaMigrator};

const NOW: i64 = 1_000_000;
const LEASE: Duration = Duration::from_secs(10);
const LEASE_MS: i64 = 10_000;

#[async_trait]
trait Harness: Send + Sync {
    fn store(&self) -> OutboxStorePort;
    async fn seed(&self, topic: &str, key: Option<&str>, payload: &str) -> String;
}

struct Memory(Arc<MemoryOutboxStore>);

#[async_trait]
impl Harness for Memory {
    fn store(&self) -> OutboxStorePort {
        self.0.clone()
    }
    async fn seed(&self, topic: &str, key: Option<&str>, payload: &str) -> String {
        self.0.push(NewMessage {
            topic: topic.to_owned(),
            key: key.map(str::to_owned),
            payload: payload.to_owned(),
        })
    }
}

fn ids(messages: &[OutboxMessage]) -> Vec<String> {
    messages.iter().map(|m| m.id.clone()).collect()
}

async fn claims_in_recording_order_and_hides_claimed_work(h: &dyn Harness) {
    let store = h.store();
    let a = h.seed("t", None, "1").await;
    let b = h.seed("t", None, "2").await;
    let c = h.seed("t", None, "3").await;

    let first = store.claim(NOW, 2, LEASE).await.unwrap();
    assert_eq!(ids(&first), [a.clone(), b.clone()]);
    assert_eq!(first[0].payload, "1");
    assert_eq!(
        ids(&store.claim(NOW, 10, LEASE).await.unwrap()),
        std::slice::from_ref(&c)
    );
    assert!(store.claim(NOW, 10, LEASE).await.unwrap().is_empty());

    // A dead relay's claims run out, and the work comes back under new claims.
    let again = store.claim(NOW + LEASE_MS + 1, 10, LEASE).await.unwrap();
    assert_eq!(ids(&again), [a, b, c]);
    assert_ne!(again[0].claim, first[0].claim);
}

async fn a_relay_that_lost_its_claim_cannot_act(h: &dyn Harness) {
    let store = h.store();
    h.seed("t", None, "x").await;
    let stale = store.claim(NOW, 1, LEASE).await.unwrap().remove(0);
    let current = store.claim(NOW + LEASE_MS + 1, 1, LEASE).await.unwrap().remove(0);

    assert!(!store.complete(&stale).await.unwrap());
    assert!(!store.release(&stale, "late", NOW).await.unwrap());
    assert!(!store.bury(&stale, "late").await.unwrap());
    assert_eq!(
        store.pending_len().await.unwrap(),
        1,
        "the current holder's work is intact"
    );

    assert!(store.complete(&current).await.unwrap());
    assert_eq!(store.pending_len().await.unwrap(), 0);
}

async fn same_key_messages_go_one_at_a_time_in_order(h: &dyn Harness) {
    let store = h.store();
    let first = h.seed("t", Some("a"), "1").await;
    let second = h.seed("t", Some("a"), "2").await;
    let other = h.seed("t", Some("b"), "1").await;
    let free = h.seed("t", None, "x").await;

    let claimed = store.claim(NOW, 10, LEASE).await.unwrap();
    assert_eq!(ids(&claimed), [first, other, free], "one per key, keyless unaffected");
    assert!(
        store.claim(NOW, 10, LEASE).await.unwrap().is_empty(),
        "the second waits"
    );

    assert!(store.complete(&claimed[0]).await.unwrap());
    assert_eq!(ids(&store.claim(NOW, 10, LEASE).await.unwrap()), [second]);
}

async fn a_failed_delivery_waits_for_its_retry_time(h: &dyn Harness) {
    let store = h.store();
    let id = h.seed("t", None, "x").await;
    let message = store.claim(NOW, 1, LEASE).await.unwrap().remove(0);

    assert!(store.release(&message, "boom", NOW + 5_000).await.unwrap());
    assert!(store.claim(NOW + 1_000, 1, LEASE).await.unwrap().is_empty());
    let retried = store.claim(NOW + 5_000, 1, LEASE).await.unwrap();
    assert_eq!(ids(&retried), [id]);
    assert_eq!(retried[0].attempts, 1);
}

async fn a_buried_message_stops_blocking_and_can_be_requeued(h: &dyn Harness) {
    let store = h.store();
    let first = h.seed("t", Some("k"), "1").await;
    let second = h.seed("t", Some("k"), "2").await;

    let message = store.claim(NOW, 1, LEASE).await.unwrap().remove(0);
    assert!(store.bury(&message, "bad").await.unwrap());
    assert_eq!(store.dead_len().await.unwrap(), 1);
    assert_eq!(store.pending_len().await.unwrap(), 1);

    let next = store.claim(NOW, 1, LEASE).await.unwrap();
    assert_eq!(ids(&next), [second], "the dead letter no longer blocks its key");

    assert!(store.requeue_dead(&first).await.unwrap());
    assert!(
        !store.requeue_dead(&first).await.unwrap(),
        "only a dead letter can be requeued"
    );
    assert_eq!(store.dead_len().await.unwrap(), 0);
    let revived = store.claim(NOW, 1, LEASE).await.unwrap();
    assert_eq!(ids(&revived), [first]);
    assert_eq!(revived[0].attempts, 0);
}

macro_rules! contract {
    ($module:ident, $harness:expr) => {
        mod $module {
            use super::*;
            macro_rules! case {
                ($name:ident) => {
                    #[tokio::test]
                    async fn $name() {
                        let harness = $harness.await;
                        super::$name(&harness).await;
                    }
                };
            }
            case!(claims_in_recording_order_and_hides_claimed_work);
            case!(a_relay_that_lost_its_claim_cannot_act);
            case!(same_key_messages_go_one_at_a_time_in_order);
            case!(a_failed_delivery_waits_for_its_retry_time);
            case!(a_buried_message_stops_blocking_and_can_be_requeued);
        }
    };
}

struct Migrator;

impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![Box::new(CreateOutbox)]
    }
}

struct Sea {
    database: Arc<Database>,
    store: Arc<SeaOutboxStore>,
}

#[async_trait]
impl Harness for Sea {
    fn store(&self) -> OutboxStorePort {
        self.store.clone()
    }
    async fn seed(&self, topic: &str, key: Option<&str>, payload: &str) -> String {
        let db = self.database.executor().await.unwrap();
        Outbox
            .record(
                &db,
                NewMessage {
                    topic: topic.to_owned(),
                    key: key.map(str::to_owned),
                    payload: payload.to_owned(),
                },
            )
            .await
            .unwrap()
    }
}

async fn sea() -> Sea {
    let database = Database::connect(
        &DatabaseConfig::new("sqlite::memory:"),
        Arc::new(SeaMigrator::<Migrator>::new()),
    )
    .await
    .unwrap();
    database.migrate().await.unwrap();
    let database = Arc::new(database);
    Sea {
        store: Arc::new(SeaOutboxStore::new(Arc::clone(&database))),
        database,
    }
}

async fn memory() -> Memory {
    Memory(Arc::new(MemoryOutboxStore::new()))
}

contract!(memory_store, memory());
contract!(sea_orm_store, sea());
