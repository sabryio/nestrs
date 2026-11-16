//! The relay's behaviour, on a memory store and paused time (retries take virtual seconds).

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nestrs::async_trait;
use nestrs_outbox::{
    MemoryOutboxStore, NewMessage, OutboxConfig, OutboxMessage, OutboxPublisher, OutboxRelay, OutboxStore, PublishError,
};

/// A publisher whose failures are scripted per test.
#[derive(Default)]
struct Scripted {
    delivered: Mutex<Vec<String>>,
    attempts: Mutex<HashMap<String, u32>>,
    /// Fail this many times per message before succeeding.
    fail_first: Mutex<u32>,
    /// These payloads fail exactly once.
    fail_once: Mutex<HashSet<String>>,
    fatal: AtomicBool,
    panic_on: Mutex<Option<String>>,
    hang: AtomicBool,
}

#[async_trait]
impl OutboxPublisher for Scripted {
    async fn publish(&self, message: &OutboxMessage) -> Result<(), PublishError> {
        if self.hang.load(Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
        if self.panic_on.lock().unwrap().as_deref() == Some(message.payload.as_str()) {
            panic!("publisher bug");
        }
        if self.fatal.load(Ordering::SeqCst) {
            return Err(PublishError::Fatal("rejected".to_owned()));
        }
        let n = {
            let mut attempts = self.attempts.lock().unwrap();
            let n = attempts.entry(message.id.clone()).or_default();
            *n += 1;
            *n
        };
        if n <= *self.fail_first.lock().unwrap() || self.fail_once.lock().unwrap().remove(&message.payload) {
            return Err(PublishError::Retry("down".to_owned()));
        }
        self.delivered.lock().unwrap().push(message.payload.clone());
        Ok(())
    }
}

fn config() -> OutboxConfig {
    OutboxConfig {
        poll_interval: Duration::from_millis(100),
        batch_size: 10,
        max_attempts: 4,
        backoff_base: Duration::from_secs(1),
        backoff_max: Duration::from_secs(8),
        publish_timeout: Duration::from_secs(1),
        ..OutboxConfig::default()
    }
}

fn setup() -> (Arc<MemoryOutboxStore>, Arc<Scripted>, OutboxRelay) {
    let store = Arc::new(MemoryOutboxStore::new());
    let publisher = Arc::new(Scripted::default());
    let relay = OutboxRelay::new(store.clone(), publisher.clone(), config());
    (store, publisher, relay)
}

fn push(store: &MemoryOutboxStore, key: Option<&str>, payload: &str) -> String {
    store.push(NewMessage {
        topic: "t".to_owned(),
        key: key.map(str::to_owned),
        payload: payload.to_owned(),
    })
}

fn delivered(publisher: &Scripted) -> Vec<String> {
    publisher.delivered.lock().unwrap().clone()
}

#[tokio::test(start_paused = true)]
async fn delivers_everything_and_forgets_it() {
    let (store, publisher, relay) = setup();
    for payload in ["a", "b", "c"] {
        push(&store, None, payload);
    }
    assert_eq!(relay.deliver_once().await, 3);
    assert_eq!(delivered(&publisher), ["a", "b", "c"]);
    assert_eq!(store.pending_len().await.unwrap(), 0);
    assert_eq!(relay.stats().published(), 3);
}

#[tokio::test(start_paused = true)]
async fn failures_back_off_exponentially_and_then_succeed() {
    let (store, publisher, relay) = setup();
    *publisher.fail_first.lock().unwrap() = 2;
    push(&store, None, "x");

    assert_eq!(relay.deliver_once().await, 1, "first attempt fails");
    assert_eq!(relay.deliver_once().await, 0, "and is not retried at once");
    tokio::time::advance(Duration::from_secs(1)).await;
    assert_eq!(relay.deliver_once().await, 1, "second attempt, after 1s, fails");
    tokio::time::advance(Duration::from_secs(1)).await;
    assert_eq!(relay.deliver_once().await, 0, "the next delay is 2s, not 1s");
    tokio::time::advance(Duration::from_secs(1)).await;
    assert_eq!(relay.deliver_once().await, 1, "third attempt succeeds");

    assert_eq!(delivered(&publisher), ["x"]);
    assert_eq!(relay.stats().retried(), 2);
    assert_eq!(store.pending_len().await.unwrap(), 0);
}

#[tokio::test(start_paused = true)]
async fn a_message_that_keeps_failing_is_buried_with_its_reason() {
    let (store, publisher, relay) = setup();
    *publisher.fail_first.lock().unwrap() = 100;
    let id = push(&store, None, "x");

    for _ in 0..config().max_attempts {
        relay.deliver_once().await;
        tokio::time::advance(Duration::from_secs(10)).await;
    }
    assert_eq!(store.dead_len().await.unwrap(), 1);
    assert_eq!(store.pending_len().await.unwrap(), 0);
    assert_eq!(relay.stats().buried(), 1);
    assert_eq!(store.last_error(&id).as_deref(), Some("down"));
    assert_eq!(relay.deliver_once().await, 0, "buried messages are left alone");
}

#[tokio::test(start_paused = true)]
async fn a_rejected_message_is_buried_at_once_and_can_be_requeued() {
    let (store, publisher, relay) = setup();
    publisher.fatal.store(true, Ordering::SeqCst);
    let id = push(&store, None, "x");

    relay.deliver_once().await;
    assert_eq!(store.dead_len().await.unwrap(), 1);
    assert_eq!(relay.stats().retried(), 0, "no retry for a fatal error");

    publisher.fatal.store(false, Ordering::SeqCst);
    assert!(store.requeue_dead(&id).await.unwrap());
    relay.deliver_once().await;
    assert_eq!(delivered(&publisher), ["x"]);
}

#[tokio::test(start_paused = true)]
async fn messages_with_one_key_stay_in_order_even_when_the_first_fails() {
    let (store, publisher, relay) = setup();
    publisher.fail_once.lock().unwrap().insert("first".to_owned());
    push(&store, Some("order-1"), "first");
    push(&store, Some("order-1"), "second");

    relay.deliver_once().await;
    assert!(
        delivered(&publisher).is_empty(),
        "the second must not overtake the first"
    );
    tokio::time::advance(Duration::from_secs(1)).await;
    relay.deliver_once().await;
    relay.deliver_once().await;
    assert_eq!(delivered(&publisher), ["first", "second"]);
}

#[tokio::test(start_paused = true)]
async fn a_publisher_that_hangs_times_out_and_counts_as_a_failure() {
    let (store, publisher, relay) = setup();
    publisher.hang.store(true, Ordering::SeqCst);
    push(&store, None, "x");

    assert_eq!(relay.deliver_once().await, 1);
    assert_eq!(relay.stats().retried(), 1);
    assert_eq!(
        store.pending_len().await.unwrap(),
        1,
        "still there for the next attempt"
    );
}

#[tokio::test(start_paused = true)]
async fn a_publisher_that_panics_does_not_take_the_relay_down() {
    let (store, publisher, relay) = setup();
    *publisher.panic_on.lock().unwrap() = Some("bad".to_owned());
    push(&store, None, "bad");
    push(&store, None, "good");

    assert_eq!(relay.deliver_once().await, 2);
    assert_eq!(delivered(&publisher), ["good"]);
    assert_eq!(relay.stats().retried(), 1);
}

#[tokio::test(start_paused = true)]
async fn the_background_task_delivers_and_stops_on_request() {
    let (store, publisher, relay) = setup();
    relay.start();
    relay.start(); // a second start changes nothing
    push(&store, None, "late");

    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(delivered(&publisher), ["late"]);

    relay.stop().await;
    push(&store, None, "after stop");
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(delivered(&publisher), ["late"], "a stopped relay delivers nothing");
}

#[test]
fn backoff_doubles_and_is_capped() {
    let config = config();
    let delays: Vec<_> = (0..6).map(|n| config.backoff(n).as_secs()).collect();
    assert_eq!(delays, [1, 2, 4, 8, 8, 8]);
}
