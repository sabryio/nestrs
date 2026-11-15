//! The jobs adapters against a real `redis-server`. Skipped when it is not installed.

use std::time::Duration;

use nestrs_jobs::{Envelope, JobLock, QueueBackend};
use nestrs_redis::{RedisJobLock, RedisQueueBackend};

mod common;
use common::Server;

macro_rules! redis_or_skip {
    () => {{
        let Some(server) = Server::start() else {
            eprintln!("redis-server not installed: skipping");
            return;
        };
        let connection = server.connect().await;
        (server, connection)
    }};
}

fn job(id: &str) -> Envelope {
    Envelope {
        id: id.to_owned(),
        payload: format!("\"{id}\""),
        attempts: 0,
    }
}

#[tokio::test]
async fn the_lock_is_shared_between_instances_and_expires() {
    let (_server, connection) = redis_or_skip!();
    let (one, two) = (RedisJobLock::new(connection.clone()), RedisJobLock::new(connection));

    assert!(one.try_acquire("tick", Duration::from_millis(300)).await.unwrap());
    assert!(!two.try_acquire("tick", Duration::from_millis(300)).await.unwrap());
    assert!(two.try_acquire("other", Duration::from_millis(300)).await.unwrap());

    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(
        two.try_acquire("tick", Duration::from_millis(300)).await.unwrap(),
        "expired"
    );
}

#[tokio::test]
async fn work_is_fifo_and_reserved_work_is_hidden_until_settled() {
    let (_server, connection) = redis_or_skip!();
    let queue = RedisQueueBackend::new(connection);
    queue.push("q", job("a")).await.unwrap();
    queue.push("q", job("b")).await.unwrap();
    assert_eq!(queue.len("q").await.unwrap(), 2);

    let first = queue.reserve("q", Duration::from_secs(1)).await.unwrap().unwrap();
    assert_eq!(first.envelope.id, "a");
    assert_eq!(queue.len("q").await.unwrap(), 1, "reserved work is not waiting");

    queue.ack("q", &first).await.unwrap();
    let second = queue.reserve("q", Duration::from_secs(1)).await.unwrap().unwrap();
    assert_eq!(second.envelope.id, "b");
    queue.ack("q", &second).await.unwrap();

    assert!(
        queue.reserve("q", Duration::from_millis(100)).await.unwrap().is_none(),
        "empty waits then gives up"
    );
}

#[tokio::test]
async fn a_blocked_reserve_wakes_when_work_arrives() {
    let (_server, connection) = redis_or_skip!();
    let queue = std::sync::Arc::new(RedisQueueBackend::new(connection));
    let waiting = {
        let queue = queue.clone();
        tokio::spawn(async move { queue.reserve("q", Duration::from_secs(5)).await })
    };
    tokio::time::sleep(Duration::from_millis(150)).await;
    queue.push("q", job("late")).await.unwrap();
    let reserved = tokio::time::timeout(Duration::from_secs(2), waiting)
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(reserved.envelope.id, "late");
}

#[tokio::test]
async fn retry_requeues_with_the_new_attempt_count_and_bury_keeps_it_for_inspection() {
    let (_server, connection) = redis_or_skip!();
    let queue = RedisQueueBackend::new(connection);
    queue.push("q", job("a")).await.unwrap();

    let reserved = queue.reserve("q", Duration::from_secs(1)).await.unwrap().unwrap();
    let mut again = reserved.envelope.clone();
    again.attempts = 1;
    queue.retry("q", &reserved, again).await.unwrap();

    let reserved = queue.reserve("q", Duration::from_secs(1)).await.unwrap().unwrap();
    assert_eq!(reserved.envelope.attempts, 1);
    queue.bury("q", &reserved).await.unwrap();

    assert_eq!(queue.len("q").await.unwrap(), 0);
    assert_eq!(queue.dead_len("q").await.unwrap(), 1);
}

#[tokio::test]
async fn work_held_by_a_crashed_worker_comes_back_on_recovery() {
    let (_server, connection) = redis_or_skip!();
    let queue = RedisQueueBackend::new(connection.clone());
    queue.push("q", job("a")).await.unwrap();
    let _held_then_lost = queue.reserve("q", Duration::from_secs(1)).await.unwrap().unwrap();
    assert_eq!(queue.len("q").await.unwrap(), 0);

    // A fresh process starts and recovers.
    let restarted = RedisQueueBackend::new(connection);
    assert_eq!(restarted.recover("q").await.unwrap(), 1);
    let reserved = restarted.reserve("q", Duration::from_secs(1)).await.unwrap().unwrap();
    assert_eq!(reserved.envelope.id, "a");
}

#[tokio::test]
async fn value_store_round_trips_expires_and_removes() {
    use nestrs_cache::ValueStore;
    let (_server, connection) = redis_or_skip!();
    let store = nestrs_redis::RedisValueStore::new(connection);

    assert_eq!(store.get("k").await.unwrap(), None);
    store
        .put("k", "\"v\"".into(), Duration::from_millis(300))
        .await
        .unwrap();
    assert_eq!(store.get("k").await.unwrap().as_deref(), Some("\"v\""));
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(store.get("k").await.unwrap(), None, "expired");

    store.put("k", "1".into(), Duration::from_secs(5)).await.unwrap();
    store.remove("k").await.unwrap();
    store.remove("k").await.unwrap();
    assert_eq!(store.get("k").await.unwrap(), None);
}
