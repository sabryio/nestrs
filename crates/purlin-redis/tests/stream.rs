//! The Redis Streams feed against a real `redis-server` (skipped when it is not installed).

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::Server;
use purlin_redis::{RedisConnection, RedisEventStream, StreamId};

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

async fn feed(redis: &RedisConnection) -> RedisEventStream {
    RedisEventStream::new(redis.clone(), "events", 1000).await.unwrap()
}

async fn next_text(subscription: &mut purlin_redis::StreamSubscription) -> String {
    let entry = tokio::time::timeout(Duration::from_secs(5), subscription.next())
        .await
        .expect("an entry within 5s")
        .expect("the feed is alive");
    String::from_utf8(entry.payload).unwrap()
}

#[tokio::test]
async fn a_live_subscriber_sees_only_what_is_published_after_it_subscribed() {
    let (_server, redis) = redis_or_skip!();
    let events = feed(&redis).await;
    events.publish(b"before").await.unwrap();

    let mut subscription = events.subscribe(None).await.unwrap();
    events.publish(b"after-1").await.unwrap();
    events.publish(b"after-2").await.unwrap();
    assert_eq!(next_text(&mut subscription).await, "after-1");
    assert_eq!(next_text(&mut subscription).await, "after-2");
}

#[tokio::test]
async fn resuming_replays_what_was_missed_then_goes_live_without_a_seam() {
    let (_server, redis) = redis_or_skip!();
    let events = feed(&redis).await;
    let first = events.publish(b"one").await.unwrap();
    events.publish(b"two").await.unwrap();
    events.publish(b"three").await.unwrap();

    let mut resumed = events.subscribe(Some(first)).await.unwrap();
    events.publish(b"four").await.unwrap(); // live, after the subscription exists
    let mut seen = Vec::new();
    for _ in 0..3 {
        seen.push(next_text(&mut resumed).await);
    }
    assert_eq!(seen, ["two", "three", "four"]);
}

#[tokio::test]
async fn long_histories_replay_across_pages_in_order_without_duplicates() {
    let (_server, redis) = redis_or_skip!();
    let events = feed(&redis).await;
    for n in 0..1200 {
        events.publish(format!("e{n}").as_bytes()).await.unwrap();
    }
    let mut subscription = events.subscribe(Some(StreamId::START)).await.unwrap();
    // MAXLEN ~ 1000 keeps at least 1000 entries; replay is best effort beyond that.
    let mut last = None;
    let mut count = 0;
    while let Ok(Some(entry)) = tokio::time::timeout(Duration::from_millis(300), subscription.next()).await {
        assert!(
            last.is_none_or(|previous| entry.id > previous),
            "strictly increasing ids"
        );
        last = Some(entry.id);
        count += 1;
    }
    assert!((1000..=1200).contains(&count), "{count} entries replayed");
    assert_eq!(String::from_utf8(events_last(&redis).await).unwrap(), "e1199");
}

async fn events_last(redis: &RedisConnection) -> Vec<u8> {
    let events = feed(redis).await;
    let mut subscription = events.subscribe(Some(StreamId::START)).await.unwrap();
    let mut last = Vec::new();
    while let Ok(Some(entry)) = tokio::time::timeout(Duration::from_millis(300), subscription.next()).await {
        last = entry.payload;
    }
    last
}

#[tokio::test]
async fn two_instances_see_each_others_events_in_the_same_order() {
    let (_server, redis) = redis_or_skip!();
    let (a, b) = (feed(&redis).await, feed(&redis).await);
    let mut on_a = a.subscribe(None).await.unwrap();
    let mut on_b = b.subscribe(None).await.unwrap();

    a.publish(b"from-a").await.unwrap();
    b.publish(b"from-b").await.unwrap();

    let a_saw = [next_text(&mut on_a).await, next_text(&mut on_a).await];
    let b_saw = [next_text(&mut on_b).await, next_text(&mut on_b).await];
    assert_eq!(a_saw, b_saw, "one global order");
    let mut sorted = a_saw.to_vec();
    sorted.sort();
    assert_eq!(sorted, ["from-a", "from-b"]);
}

#[tokio::test]
async fn detached_publishes_keep_their_order_and_never_block_the_caller() {
    let (_server, redis) = redis_or_skip!();
    let events = feed(&redis).await;
    let mut subscription = events.subscribe(None).await.unwrap();
    for n in 0..50 {
        events.publish_detached(format!("d{n}").into_bytes());
    }
    for n in 0..50 {
        assert_eq!(next_text(&mut subscription).await, format!("d{n}"));
    }
}

#[tokio::test]
async fn many_local_subscribers_share_one_reader() {
    let (_server, redis) = redis_or_skip!();
    let events = Arc::new(feed(&redis).await);
    let mut subscribers = Vec::new();
    for _ in 0..20 {
        subscribers.push(events.subscribe(None).await.unwrap());
    }
    events.publish(b"broadcast").await.unwrap();
    for subscriber in &mut subscribers {
        assert_eq!(next_text(subscriber).await, "broadcast");
    }
}

#[tokio::test]
async fn the_reader_survives_a_redis_restart_and_carries_on() {
    let (mut server, redis) = redis_or_skip!();
    let events = feed(&redis).await;
    let mut subscription = events.subscribe(None).await.unwrap();
    events.publish(b"one").await.unwrap();
    assert_eq!(next_text(&mut subscription).await, "one");

    // Restart Redis on the same port: the stream is empty again (no persistence), but the reader
    // reconnects and new entries flow.
    let port = server.port;
    server.child.kill().unwrap();
    let _ = server.child.wait();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let _restarted = Server::start_on(port).expect("restart");
    for _ in 0..50 {
        if redis.ping().await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    for _ in 0..50 {
        if events.publish(b"two").await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    // The new stream's ids restart, so an old cursor would be ahead of them: the subscriber
    // may skip entries until ids pass its position. Entries with larger ids still arrive.
    for _ in 0..3 {
        events.publish(b"three").await.unwrap();
    }
    let got = tokio::time::timeout(Duration::from_secs(10), subscription.next()).await;
    assert!(
        matches!(got, Ok(Some(_))),
        "the feed kept flowing after the restart: {got:?}"
    );
}
