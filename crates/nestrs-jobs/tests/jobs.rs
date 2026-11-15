//! The runner end to end through `nestrs-testing`, on paused time.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use nestrs::prelude::*;
use nestrs_jobs::*;
use nestrs_testing::TestApp;

/// Shared counters the test reads.
#[derive(Default)]
struct Probe {
    ticks: AtomicU32,
    attempts: AtomicU32,
    handled: Mutex<Vec<String>>,
    slow_runs: AtomicU32,
}

#[injectable(contributes = JobPort)]
struct Tick {
    probe: Arc<Probe>,
}

#[async_trait]
impl Job for Tick {
    fn name(&self) -> &str {
        "tick"
    }
    fn schedule(&self) -> Schedule {
        Schedule::every(Duration::from_secs(10)).unwrap()
    }
    async fn run(&self) -> Result<(), JobError> {
        let n = self.probe.ticks.fetch_add(1, Ordering::SeqCst) + 1;
        match n {
            2 => Err(JobError::new("boom")),
            3 => panic!("kaboom"),
            _ => Ok(()),
        }
    }
}

#[injectable(contributes = QueueHandlerPort)]
struct Mailer {
    probe: Arc<Probe>,
}

#[async_trait]
impl QueueHandler for Mailer {
    fn queue(&self) -> &str {
        "mail"
    }
    fn max_attempts(&self) -> u32 {
        3
    }
    fn backoff(&self, _: u32) -> Duration {
        Duration::from_secs(1)
    }
    async fn handle(&self, payload: &str) -> Result<(), HandlerError> {
        let to: String = decode(payload)?;
        self.probe.attempts.fetch_add(1, Ordering::SeqCst);
        match to.as_str() {
            "flaky@example.com" if self.probe.attempts.load(Ordering::SeqCst) < 3 => {
                Err(HandlerError::Retry("smtp down".into()))
            }
            "down@example.com" => Err(HandlerError::Retry("smtp down".into())),
            _ => {
                self.probe.handled.lock().unwrap().push(to);
                Ok(())
            }
        }
    }
}

/// A handler that takes far longer than the lease.
#[injectable(contributes = QueueHandlerPort)]
struct Slow {
    probe: Arc<Probe>,
}

#[async_trait]
impl QueueHandler for Slow {
    fn queue(&self) -> &str {
        "slow"
    }
    fn timeout(&self) -> Duration {
        Duration::from_secs(60)
    }
    async fn handle(&self, _: &str) -> Result<(), HandlerError> {
        self.probe.slow_runs.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_secs(10)).await;
        Ok(())
    }
}

#[module(
    imports = [JobsModule::in_memory()],
    providers = [Tick, Mailer, Slow, Provider::value(Probe::default())],
)]
struct AppModule;

async fn settle() {
    // Let spawned tasks run, then move the clock; paused time auto-advances when idle.
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
}

#[tokio::test(start_paused = true)]
async fn a_job_runs_on_schedule_and_survives_errors_and_panics() {
    let app = TestApp::start(AppModule).await;
    let probe = app.resolve::<Probe>();

    for _ in 0..4 {
        tokio::time::sleep(Duration::from_secs(10)).await;
        settle().await;
    }
    // Tick 2 returned an error and tick 3 panicked; ticks 4+ still happened.
    assert!(
        probe.ticks.load(Ordering::SeqCst) >= 4,
        "{}",
        probe.ticks.load(Ordering::SeqCst)
    );
    assert!(app.shutdown().await.is_empty());
}

#[tokio::test(start_paused = true)]
async fn queued_work_is_handled_retried_and_buried() {
    let app = TestApp::start(AppModule).await;
    let probe = app.resolve::<Probe>();
    let queue = app.resolve::<Queue>();
    let backend = app.resolve::<QueueBackendPort>();

    queue.enqueue("mail", &"ada@example.com").await.unwrap();
    queue.enqueue("mail", &"flaky@example.com").await.unwrap();
    queue.enqueue("mail", &"down@example.com").await.unwrap();

    for _ in 0..30 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        settle().await;
    }

    let mut handled = probe.handled.lock().unwrap().clone();
    handled.sort();
    assert_eq!(
        handled,
        ["ada@example.com", "flaky@example.com"],
        "the flaky one succeeded on a retry"
    );
    assert_eq!(
        backend.dead_len("mail").await.unwrap(),
        1,
        "the permanently failing one was buried"
    );
    assert_eq!(backend.len("mail").await.unwrap(), 0);
}

#[tokio::test(start_paused = true)]
async fn an_undecodable_payload_is_buried_without_retrying() {
    let app = TestApp::start(AppModule).await;
    let queue = app.resolve::<Queue>();
    let backend = app.resolve::<QueueBackendPort>();
    let probe = app.resolve::<Probe>();

    queue.enqueue("mail", &42).await.unwrap(); // not a string

    for _ in 0..5 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        settle().await;
    }
    assert_eq!(backend.dead_len("mail").await.unwrap(), 1);
    assert_eq!(
        probe.attempts.load(Ordering::SeqCst),
        0,
        "decoding failed before any attempt counted"
    );
}

#[tokio::test(start_paused = true)]
async fn disabled_jobs_start_nothing() {
    let app = TestApp::builder(AppModule)
        .override_value(JobsConfig {
            enabled: false,
            ..JobsConfig::default()
        })
        .start()
        .await;
    tokio::time::sleep(Duration::from_secs(60)).await;
    settle().await;
    assert_eq!(app.resolve::<Probe>().ticks.load(Ordering::SeqCst), 0);
}

fn short_leases() -> JobsConfig {
    JobsConfig {
        lease: Duration::from_secs(3),
        reap_every: Duration::from_secs(1),
        ..JobsConfig::default()
    }
}

#[tokio::test(start_paused = true)]
async fn a_long_job_keeps_its_lease_and_runs_exactly_once() {
    let app = TestApp::builder(AppModule).override_value(short_leases()).start().await;
    let probe = app.resolve::<Probe>();
    let backend = app.resolve::<QueueBackendPort>();
    app.resolve::<Queue>().enqueue("slow", &1).await.unwrap();

    // Ten seconds of work under a three second lease: only renewals keep it ours.
    for _ in 0..14 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        settle().await;
    }
    assert_eq!(
        probe.slow_runs.load(Ordering::SeqCst),
        1,
        "never reaped, never duplicated"
    );
    assert_eq!(backend.leased_len("slow").await.unwrap(), 0, "acked at the end");
    assert_eq!(backend.dead_len("slow").await.unwrap(), 0);
}

#[tokio::test(start_paused = true)]
async fn a_worker_that_dies_loses_its_work_to_the_reaper_and_poison_is_buried() {
    let app = TestApp::builder(AppModule).override_value(short_leases()).start().await;
    let backend = app.resolve::<QueueBackendPort>();
    let queue = "orphans";
    let envelope = |id: &str| Envelope {
        id: id.into(),
        payload: "1".into(),
        attempts: 0,
    };

    backend.push(queue, envelope("a")).await.unwrap();
    // A worker takes it and then vanishes: no renew, no ack.
    let _lost = backend
        .reserve(queue, Duration::from_secs(1), Duration::from_secs(3))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(backend.reap(queue, 5).await.unwrap(), 0, "a live lease is left alone");

    tokio::time::sleep(Duration::from_secs(4)).await;
    assert_eq!(backend.reap(queue, 5).await.unwrap(), 1);
    let again = backend
        .reserve(queue, Duration::from_secs(1), Duration::from_secs(3))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(again.envelope.attempts, 1, "the lost attempt counts");

    // A late ack from the dead worker must not erase what moved on.
    backend.ack(queue, &_lost).await.unwrap();
    assert_eq!(
        backend.leased_len(queue).await.unwrap(),
        1,
        "the new holder still has it"
    );

    // Work that keeps killing its worker is buried once it reaches max_attempts.
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert_eq!(backend.reap(queue, 2).await.unwrap(), 1);
    assert_eq!(backend.dead_len(queue).await.unwrap(), 1);
    assert_eq!(backend.len(queue).await.unwrap(), 0);
}
