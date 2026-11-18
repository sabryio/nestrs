//! Soak: every protective interceptor under concurrent mixed load, then invariants.
//!
//! Runs for `SOAK_SECS` seconds (default 3, so it is cheap in CI); for a real soak:
//!
//! ```text
//! SOAK_SECS=600 SOAK_WORKERS=64 cargo test -p purlin-bench --release --test soak -- --nocapture
//! ```
//!
//! The invariants are the contracts the interceptors document, checked from the outside:
//! nothing panics; every status is one the route may produce; the bulkhead never admits
//! more than its capacity; an idempotency key never runs a write twice; the cache stays
//! bounded and spares the handler; the circuit breaker recovers; metrics count every
//! request exactly once and end with nothing in flight; request ids are unique.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use http_body_util::BodyExt;
use purlin::http::axum::Router;
use purlin::http::axum::body::Body;
use purlin::http::axum::http::{Request, StatusCode};
use purlin_bench::soak::{self, BULKHEAD};
use tower::ServiceExt;

/// A tiny xorshift: the soak must not depend on a rand crate, and a seed makes a failing
/// run reproducible.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn env_or(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

#[derive(Default)]
struct Tally {
    /// (route, status) → count, as seen by the client.
    seen: HashMap<(&'static str, u16), u64>,
    request_ids: Vec<String>,
    /// Idempotency key → bodies it was answered with.
    writes: HashMap<u64, HashSet<String>>,
    latencies_ok: Vec<Duration>,
}

async fn send(router: &Router, method: &str, path: &str, key: Option<u64>) -> (u16, String, Option<String>, Duration) {
    let mut request = Request::builder().method(method).uri(path);
    if let Some(key) = key {
        request = request.header("idempotency-key", format!("k{key}"));
    }
    let started = Instant::now();
    let response = router
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status().as_u16();
    let id = response
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        String::from_utf8_lossy(&body).into_owned(),
        id,
        started.elapsed(),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_full_interceptor_stack_holds_its_contracts_under_load() {
    let duration = Duration::from_secs(env_or("SOAK_SECS", 3));
    let workers = env_or("SOAK_WORKERS", 32);
    let seed = env_or("SOAK_SEED", 0x9E37_79B9_7F4A_7C15);
    eprintln!("soak: {workers} workers for {duration:?}, seed {seed:#x}");

    let soak = soak::build().await;
    soak.probe.flaky.store(true, Ordering::SeqCst);
    let tally = Arc::new(Mutex::new(Tally::default()));
    let stop = Arc::new(AtomicBool::new(false));

    let mut tasks = Vec::new();
    for worker in 0..workers {
        let (router, tally, stop) = (soak.router.clone(), tally.clone(), stop.clone());
        tasks.push(tokio::spawn(async move {
            let mut rng = Rng(seed ^ (worker + 1).wrapping_mul(0x2545_F491_4F6C_DD1D));
            let mut local = Tally::default();
            while !stop.load(Ordering::Relaxed) {
                // Handlers that never wait would otherwise starve the timer that ends the run.
                tokio::task::yield_now().await;
                let (route, method, path, key): (&'static str, &str, &str, Option<u64>) = match rng.below(100) {
                    0..=29 => ("ok", "GET", "/soak/ok", None),
                    30..=44 => ("cached", "GET", "/soak/cached", None),
                    45..=59 => ("slow", "GET", "/soak/slow", None),
                    60..=74 => ("flaky", "GET", "/soak/flaky", None),
                    75..=84 => ("limited", "GET", "/soak/limited", None),
                    _ => ("write", "POST", "/soak/write", Some(rng.below(40))),
                };
                let (status, body, id, took) = send(&router, method, path, key).await;
                *local.seen.entry((route, status)).or_default() += 1;
                local.request_ids.extend(id);
                if let Some(key) = key.filter(|_| status == 201) {
                    local.writes.entry(key).or_default().insert(body);
                }
                if route == "ok" && status == 200 {
                    local.latencies_ok.push(took);
                }
            }
            let mut shared = tally.lock().unwrap();
            for (cell, count) in local.seen {
                *shared.seen.entry(cell).or_default() += count;
            }
            shared.request_ids.extend(local.request_ids);
            for (key, bodies) in local.writes {
                shared.writes.entry(key).or_default().extend(bodies);
            }
            shared.latencies_ok.extend(local.latencies_ok);
        }));
    }

    // First third: the downstream is failing. Then it heals.
    tokio::time::sleep(duration / 3).await;
    soak.probe.flaky.store(false, Ordering::SeqCst);
    tokio::time::sleep(duration - duration / 3).await;
    stop.store(true, Ordering::Relaxed);
    for task in tasks {
        task.await.expect("a worker panicked");
    }

    let tally = std::mem::take(&mut *tally.lock().unwrap());
    let total: u64 = tally.seen.values().sum();
    eprintln!(
        "soak: {total} requests ({:.0}/s)",
        total as f64 / duration.as_secs_f64()
    );
    for ((route, status), count) in {
        let mut rows: Vec<_> = tally.seen.iter().collect();
        rows.sort();
        rows
    } {
        eprintln!("  {route:<8} {status}  {count}");
    }

    // 1. Only statuses a route may produce.
    let allowed: HashMap<&str, &[u16]> = HashMap::from([
        ("ok", &[200][..]),
        ("cached", &[200]),
        ("slow", &[200, 503]),
        ("flaky", &[200, 500, 503]),
        ("limited", &[200, 429]),
        ("write", &[201, 409]),
    ]);
    for ((route, status), count) in &tally.seen {
        assert!(
            allowed[route].contains(status),
            "{route} answered {status} {count} times, which it must never do"
        );
    }

    // 2. The bulkhead never admitted more than its capacity.
    let peak = soak.probe.slow_peak.load(Ordering::SeqCst);
    assert!(peak <= BULKHEAD, "{peak} requests were inside a bulkhead of {BULKHEAD}");
    assert_eq!(
        soak.probe.slow_now.load(Ordering::SeqCst),
        0,
        "no request is stuck in the handler"
    );

    // 3. A key never ran a write twice: replays returned the original response.
    for (key, bodies) in &tally.writes {
        assert_eq!(bodies.len(), 1, "key k{key} produced different responses: {bodies:?}");
    }
    let writes_run = soak.probe.writes_run.load(Ordering::SeqCst);
    assert!(writes_run <= 40, "{writes_run} writes ran for 40 distinct keys");

    // 4. The cache spared the handler.
    let cached_requests: u64 = tally
        .seen
        .iter()
        .filter(|((r, _), _)| *r == "cached")
        .map(|(_, n)| n)
        .sum();
    let cached_runs = soak.probe.cached_runs.load(Ordering::SeqCst);
    assert!(
        cached_runs * 4 < cached_requests,
        "handler ran {cached_runs} times for {cached_requests} cached requests"
    );

    // 5. The breaker opened while the downstream failed, and closed after it healed.
    let flaky = |status| tally.seen.get(&("flaky", status)).copied().unwrap_or(0);
    assert!(
        flaky(500) > 0 && flaky(503) > 0,
        "the breaker should have seen failures and then refused"
    );
    tokio::time::sleep(Duration::from_millis(300)).await; // longer than the cool-down
    let (status, ..) = send(&soak.router, "GET", "/soak/flaky", None).await;
    assert_eq!(
        status, 200,
        "after the cool-down the probe succeeds and the circuit closes"
    );

    // 6. Metrics counted every request once (plus the one probe above), nothing in flight.
    let rendered = soak.registry.render();
    let counted: u64 = rendered
        .lines()
        .filter(|line| line.starts_with("http_requests_total"))
        .filter_map(|line| line.rsplit(' ').next()?.parse::<u64>().ok())
        .sum();
    assert_eq!(counted, total + 1, "metrics and clients disagree");
    assert_eq!(soak.registry.in_flight(), 0, "the in-flight gauge returned to zero");

    // 7. Request ids are unique.
    let unique: HashSet<_> = tally.request_ids.iter().collect();
    assert_eq!(unique.len(), tally.request_ids.len(), "duplicate x-request-id");
    assert_eq!(tally.request_ids.len() as u64, total, "every response carried an id");

    // 8. Latency stays sane for an endpoint with no protection of its own.
    let mut latencies = tally.latencies_ok.clone();
    latencies.sort();
    let p99 = latencies[latencies.len() * 99 / 100];
    eprintln!("soak: /soak/ok p99 = {p99:?}");
    assert!(p99 < Duration::from_secs(1), "p99 of a trivial route was {p99:?}");

    // Silence an unused-import warning when assertions are compiled out.
    let _ = StatusCode::OK;
}
