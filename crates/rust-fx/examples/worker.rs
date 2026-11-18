//! A worker with no web stack at all: a clock, a job queue, two jobs and a graceful
//! shutdown, wired with `fx`.
//!
//! ```text
//! cargo run -p rust-fx --example worker
//! ```
#![allow(missing_docs)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use fx::{App, Group, Lifecycle, Shutdowner};

// ---- ports ---------------------------------------------------------------------

trait Clock: Send + Sync {
    fn tick(&self) -> u32;
}

trait Job: Send + Sync {
    fn name(&self) -> &'static str;
    fn run(&self, now: u32);
}

// ---- adapters ------------------------------------------------------------------

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
struct Report;

#[fx::component]
impl Cleanup {
    #[fx::constructor]
    fn new() -> Self {
        Self
    }
}

#[fx::component]
impl Report {
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
    fn run(&self, now: u32) {
        println!("[{now}] {}: sweeping temp files", self.name());
    }
}

#[fx::bind(group)]
impl Job for Report {
    fn name(&self) -> &'static str {
        "report"
    }
    fn run(&self, now: u32) {
        println!("[{now}] {}: sending the daily report", self.name());
    }
}

// ---- modules -------------------------------------------------------------------

#[fx::module(providers = [CounterClock], exports = [dyn Clock])]
struct TimeModule;

#[fx::module(providers = [Cleanup, Report])]
struct JobsModule;

// ---- the worker itself -----------------------------------------------------------

/// Runs every job every 50 ms, three times, then asks the app to stop.
fn work(clock: Arc<dyn Clock>, jobs: Group<dyn Job>, life: Lifecycle, shutdown: Shutdowner) {
    let handle = tokio::spawn(async move {
        for _ in 0..3 {
            let now = clock.tick();
            for job in &jobs {
                job.run(now);
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        shutdown.shutdown();
    });
    life.on_stop(move || async move {
        handle.abort();
        println!("worker stopped");
    });
}

#[tokio::main]
async fn main() -> fx::Result<()> {
    let code = App::new()
        .module::<TimeModule>()
        .module::<JobsModule>()
        .invoke(work)
        .run()
        .await?;
    std::process::exit(code);
}
