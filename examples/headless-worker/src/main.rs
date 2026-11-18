//! A queue-less worker: no HTTP, no controllers, only `purlin::fx`.
//!
//! ```text
//! cargo run -p headless-worker
//! ```
//!
//! Shape: a port (`Clock`) bound to an adapter, a group of `Job`s, one invoke that drives
//! them, and a graceful shutdown. Swap in `purlin-jobs`, `purlin-outbox` or
//! `purlin-sea-orm` (all headless) the same way.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use purlin::fx::{self, App, Group, Lifecycle, Shutdowner};

// ---- ports -----------------------------------------------------------------------

trait Clock: Send + Sync {
    fn tick(&self) -> u32;
}

trait Job: Send + Sync {
    fn name(&self) -> &'static str;
    fn run(&self, now: u32) -> String;
}

// ---- adapters --------------------------------------------------------------------

#[derive(Default)]
struct CounterClock(AtomicU32);

#[fx::component]
impl CounterClock {
    #[fx::constructor]
    fn new() -> Self {
        Self::default()
    }
}

#[fx::bind]
impl Clock for CounterClock {
    fn tick(&self) -> u32 {
        self.0.fetch_add(1, Ordering::SeqCst)
    }
}

struct Cleanup;

#[fx::component]
impl Cleanup {
    #[fx::constructor]
    fn new() -> Self {
        Self
    }
}

#[fx::bind(group)]
impl Job for Cleanup {
    fn name(&self) -> &'static str {
        "cleanup"
    }

    fn run(&self, now: u32) -> String {
        format!("[{now}] cleanup: swept temp files")
    }
}

// ---- composition -----------------------------------------------------------------

#[fx::module(providers = [CounterClock], exports = [dyn Clock])]
struct TimeModule;

#[fx::module(providers = [Cleanup])]
struct JobsModule;

/// Reports of every job run, so a test can look at them.
#[derive(Default)]
struct Journal(std::sync::Mutex<Vec<String>>);

impl Journal {
    fn record(&self, line: String) {
        println!("{line}");
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(line);
    }

    #[cfg(test)]
    fn lines(&self) -> Vec<String> {
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }
}

/// Runs every job on a tick, `rounds` times, then asks the app to stop.
fn drive(clock: Arc<dyn Clock>, jobs: Group<dyn Job>, journal: Arc<Journal>, life: Lifecycle, stop: Shutdowner) {
    let handle = tokio::spawn(async move {
        for _ in 0..3 {
            let now = clock.tick();
            for job in &jobs {
                journal.record(format!("{} <- {}", job.name(), job.run(now)));
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        stop.shutdown_with(0);
    });
    life.on_stop(move || async move {
        handle.abort();
    });
}

fn app(journal: Arc<Journal>) -> App {
    App::new()
        .supply_arc(journal)
        .module::<TimeModule>()
        .module::<JobsModule>()
        .invoke(drive)
}

#[tokio::main]
async fn main() -> fx::Result<()> {
    let code = app(Arc::default()).run().await?;
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_worker_runs_every_job_each_tick_and_exits_cleanly() {
        let journal = Arc::new(Journal::default());
        let code = app(journal.clone()).run().await.unwrap();
        assert_eq!(code, 0);
        assert_eq!(
            journal.lines(),
            [
                "cleanup <- [0] cleanup: swept temp files",
                "cleanup <- [1] cleanup: swept temp files",
                "cleanup <- [2] cleanup: swept temp files"
            ]
        );
    }

    #[test]
    fn the_wiring_validates_without_running_anything() {
        app(Arc::default()).validate().unwrap();
    }

    #[tokio::test]
    async fn replace_swaps_the_clock_without_touching_the_modules() {
        struct Fixed;
        impl Clock for Fixed {
            fn tick(&self) -> u32 {
                7
            }
        }
        let journal = Arc::new(Journal::default());
        app(journal.clone())
            .replace::<dyn Clock>(Arc::new(Fixed))
            .run()
            .await
            .unwrap();
        assert!(
            journal.lines().iter().all(|line| line.contains("[7]")),
            "{:?}",
            journal.lines()
        );
    }
}
