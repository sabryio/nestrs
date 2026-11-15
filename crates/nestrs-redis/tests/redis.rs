//! Runs the adapters against a real `redis-server` started on a free port for each test.
//! Skipped (with a note) when the binary is not installed.

use std::sync::Arc;
use std::time::Duration;

use nestrs_health::HealthIndicator;
use nestrs_http::axum::Router;
use nestrs_http::axum::body::Body;
use nestrs_http::axum::http::{Request, StatusCode};
use nestrs_http::axum::routing::post;
use nestrs_http::{Decision, RateLimit, RateLimitStore, intercepted};
use nestrs_interceptors::{Begin, CacheStore, Idempotency, IdempotencyStore, Snapshot};
mod common;

use common::Server;
use nestrs_redis::{RedisCacheStore, RedisIdempotencyStore, RedisIndicator, RedisRateLimitStore};
use tower::ServiceExt;

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

fn snapshot(body: &'static str) -> Snapshot {
    let mut headers = nestrs_http::axum::http::HeaderMap::new();
    headers.insert("content-type", "text/plain".parse().unwrap());
    Snapshot {
        status: StatusCode::OK,
        headers,
        body: body.into(),
    }
}

// ---- rate limiting --------------------------------------------------------------

#[tokio::test]
async fn rate_limit_allows_the_burst_then_refuses_with_a_retry_after() {
    let (_server, redis) = redis_or_skip!();
    let store = RedisRateLimitStore::new(redis);
    let window = Duration::from_secs(10);
    let mut remaining = Vec::new();
    for _ in 0..3 {
        let decision = store.hit("alice", 3, window).await.unwrap();
        assert!(decision.allowed);
        remaining.push(decision.remaining);
    }
    assert_eq!(remaining, [2, 1, 0]);

    let refused: Decision = store.hit("alice", 3, window).await.unwrap();
    assert!(!refused.allowed);
    assert!(
        refused.retry_after > Duration::ZERO && refused.retry_after <= Duration::from_secs(4),
        "{refused:?}"
    );

    assert!(
        store.hit("bob", 3, window).await.unwrap().allowed,
        "keys are independent"
    );
}

#[tokio::test]
async fn rate_limit_refills_smoothly() {
    let (_server, redis) = redis_or_skip!();
    let store = RedisRateLimitStore::new(redis);
    let window = Duration::from_secs(1);
    assert!(store.hit("k", 2, window).await.unwrap().allowed);
    assert!(store.hit("k", 2, window).await.unwrap().allowed);
    assert!(!store.hit("k", 2, window).await.unwrap().allowed);
    tokio::time::sleep(Duration::from_millis(600)).await; // one request costs 500 ms
    assert!(store.hit("k", 2, window).await.unwrap().allowed);
}

#[tokio::test]
async fn rate_limit_is_atomic_under_concurrency() {
    let (_server, redis) = redis_or_skip!();
    let store = Arc::new(RedisRateLimitStore::new(redis));
    let tasks: Vec<_> = (0..40)
        .map(|_| {
            let store = store.clone();
            tokio::spawn(async move { store.hit("shared", 5, Duration::from_secs(60)).await.unwrap().allowed })
        })
        .collect();
    let mut allowed = 0;
    for task in tasks {
        allowed += usize::from(task.await.unwrap());
    }
    assert_eq!(
        allowed, 5,
        "exactly the limit gets through, however the requests interleave"
    );
}

#[tokio::test]
async fn rate_limit_keys_expire_and_prefixes_isolate_apps() {
    let (_server, redis) = redis_or_skip!();
    let a = RedisRateLimitStore::new(redis.clone()).prefix("app-a");
    let b = RedisRateLimitStore::new(redis).prefix("app-b");
    assert!(a.hit("k", 1, Duration::from_millis(200)).await.unwrap().allowed);
    assert!(!a.hit("k", 1, Duration::from_millis(200)).await.unwrap().allowed);
    assert!(
        b.hit("k", 1, Duration::from_millis(200)).await.unwrap().allowed,
        "another app's counter"
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(a.hit("k", 1, Duration::from_millis(200)).await.unwrap().allowed);
}

#[tokio::test]
async fn rate_limit_reports_an_unreachable_redis_as_a_store_error() {
    let (mut server, redis) = redis_or_skip!();
    let store = RedisRateLimitStore::new(redis);
    server.child.kill().unwrap();
    let _ = server.child.wait();
    let outcome = tokio::time::timeout(Duration::from_secs(10), store.hit("k", 1, Duration::from_secs(1))).await;
    assert!(matches!(outcome, Ok(Err(_))), "{outcome:?}");
}

// ---- cache ------------------------------------------------------------------------

#[tokio::test]
async fn cache_round_trips_and_expires() {
    let (_server, redis) = redis_or_skip!();
    let store = RedisCacheStore::new(redis);
    assert!(store.get("/posts").await.is_none());

    store
        .put("/posts".into(), snapshot("hello"), Duration::from_millis(250))
        .await;
    let hit = store.get("/posts").await.expect("hit");
    assert_eq!(hit.status, StatusCode::OK);
    assert_eq!(hit.body, "hello");
    assert_eq!(hit.headers["content-type"], "text/plain");

    tokio::time::sleep(Duration::from_millis(350)).await;
    assert!(store.get("/posts").await.is_none(), "Redis expired it");
}

#[tokio::test]
async fn cache_treats_corrupt_entries_and_outages_as_misses() {
    let (mut server, redis) = redis_or_skip!();
    let store = RedisCacheStore::new(redis.clone());

    let mut raw = redis::Client::open(format!("redis://127.0.0.1:{}/", server.port))
        .unwrap()
        .get_multiplexed_async_connection()
        .await
        .unwrap();
    let _: () = redis::cmd("SET")
        .arg("nestrs:cache:bad")
        .arg(&b"\xff\xff"[..])
        .query_async(&mut raw)
        .await
        .unwrap();
    assert!(store.get("bad").await.is_none());

    server.child.kill().unwrap();
    let _ = server.child.wait();
    let outcome = tokio::time::timeout(Duration::from_secs(10), store.get("anything")).await;
    assert!(matches!(outcome, Ok(None)));
    tokio::time::timeout(
        Duration::from_secs(10),
        store.put("k".into(), snapshot("x"), Duration::from_secs(1)),
    )
    .await
    .expect("a failed write returns, it does not hang");
}

// ---- idempotency ------------------------------------------------------------------

#[tokio::test]
async fn idempotency_walks_through_start_in_flight_replay_and_mismatch() {
    let (_server, redis) = redis_or_skip!();
    let store = RedisIdempotencyStore::new(redis);

    assert!(matches!(store.begin("k", "fp1").await, Begin::Started));
    assert!(matches!(store.begin("k", "fp1").await, Begin::InFlight));
    assert!(matches!(store.begin("k", "other").await, Begin::Mismatch));

    store.complete("k", snapshot("done")).await;
    match store.begin("k", "fp1").await {
        Begin::Replay(response) => assert_eq!(response.body, "done"),
        other => panic!("expected a replay, got {other:?}"),
    }
    assert!(matches!(store.begin("k", "other").await, Begin::Mismatch));
}

#[tokio::test]
async fn idempotency_abandon_frees_a_running_key_but_not_a_finished_one() {
    let (_server, redis) = redis_or_skip!();
    let store = RedisIdempotencyStore::new(redis);

    assert!(matches!(store.begin("a", "fp").await, Begin::Started));
    store.abandon("a").await;
    assert!(
        matches!(store.begin("a", "fp").await, Begin::Started),
        "released, so it can run again"
    );

    store.complete("a", snapshot("x")).await;
    store.abandon("a").await;
    assert!(
        matches!(store.begin("a", "fp").await, Begin::Replay(_)),
        "a finished key stays"
    );
}

#[tokio::test]
async fn idempotency_exactly_one_concurrent_claim_wins() {
    let (_server, redis) = redis_or_skip!();
    let store = Arc::new(RedisIdempotencyStore::new(redis));
    let tasks: Vec<_> = (0..30)
        .map(|_| {
            let store = store.clone();
            tokio::spawn(async move { store.begin("race", "fp").await })
        })
        .collect();
    let mut started = 0;
    for task in tasks {
        started += usize::from(matches!(task.await.unwrap(), Begin::Started));
    }
    assert_eq!(started, 1);
}

#[tokio::test]
async fn idempotency_a_dead_instances_claim_frees_itself() {
    let (_server, redis) = redis_or_skip!();
    let store = RedisIdempotencyStore::new(redis).pending_ttl(Duration::from_millis(200));
    assert!(matches!(store.begin("crashed", "fp").await, Begin::Started));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        matches!(store.begin("crashed", "fp").await, Begin::Started),
        "not locked out for 24 hours"
    );
}

#[tokio::test]
async fn idempotency_fails_closed_when_redis_is_down() {
    let (mut server, redis) = redis_or_skip!();
    let store = RedisIdempotencyStore::new(redis);
    server.child.kill().unwrap();
    let _ = server.child.wait();
    let outcome = tokio::time::timeout(Duration::from_secs(10), store.begin("k", "fp")).await;
    assert!(matches!(outcome, Ok(Begin::Unavailable)), "{outcome:?}");
}

// ---- through the interceptors -----------------------------------------------------

fn request(headers: &[(&str, &str)], body: &str) -> Request<Body> {
    let mut builder = Request::post("/");
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    builder.body(Body::from(body.to_owned())).unwrap()
}

#[tokio::test]
async fn two_instances_share_idempotency_and_rate_limits_through_redis() {
    let (_server, redis) = redis_or_skip!();
    let calls = Arc::new(std::sync::atomic::AtomicU32::new(0));

    // Two "instances": separate routers, separate interceptor values, one Redis.
    let instance = |calls: Arc<std::sync::atomic::AtomicU32>| {
        let idempotency = Idempotency::new(Arc::new(RedisIdempotencyStore::new(redis.clone())));
        let handler = post(move || {
            let n = calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            async move { (StatusCode::CREATED, format!("created #{n}")) }
        });
        Router::new().route("/", intercepted(handler, Arc::new(idempotency)))
    };
    let (one, two) = (instance(calls.clone()), instance(calls.clone()));

    let first = one
        .oneshot(request(&[("idempotency-key", "order-1")], "{}"))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::CREATED);
    let retry = two
        .oneshot(request(&[("idempotency-key", "order-1")], "{}"))
        .await
        .unwrap();
    assert_eq!(retry.status(), StatusCode::CREATED);
    assert_eq!(retry.headers()["idempotent-replayed"], "true");
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the retry on the other instance did not run the handler"
    );

    // The same for the rate limit: 2 per minute across both instances.
    let limited = |store: Arc<RedisRateLimitStore>| {
        let limit = RateLimit::per_minute(2).scope("shared").store(store);
        Router::new().route("/", intercepted(post(|| async { "ok" }), Arc::new(limit)))
    };
    let store = Arc::new(RedisRateLimitStore::new(redis));
    let (a, b) = (limited(store.clone()), limited(store));
    let statuses = [
        a.clone().oneshot(request(&[], "")).await.unwrap().status(),
        b.clone().oneshot(request(&[], "")).await.unwrap().status(),
        a.oneshot(request(&[], "")).await.unwrap().status(),
    ];
    assert_eq!(
        statuses,
        [StatusCode::OK, StatusCode::OK, StatusCode::TOO_MANY_REQUESTS]
    );
}

#[tokio::test]
async fn the_health_indicator_reports_up_then_down_and_is_non_critical_by_default() {
    let (mut server, redis) = redis_or_skip!();
    let indicator = RedisIndicator::new(redis.clone());
    assert_eq!(indicator.name(), "redis");
    assert!(!HealthIndicator::critical(&indicator));
    assert!(HealthIndicator::critical(&RedisIndicator::new(redis).required()));
    assert!(indicator.check().await.is_ok());

    server.child.kill().unwrap();
    let _ = server.child.wait();
    let outcome = tokio::time::timeout(Duration::from_secs(10), indicator.check())
        .await
        .unwrap();
    assert_eq!(
        outcome.unwrap_err().to_string(),
        "redis unreachable",
        "no hostnames in the message"
    );
}
