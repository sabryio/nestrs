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

#[module(
    imports = [JobsModule::in_memory()],
    providers = [Tick, Mailer, Provider::value(Probe::default())],
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
