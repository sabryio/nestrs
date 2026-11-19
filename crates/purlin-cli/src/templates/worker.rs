//! `purlin new --kind worker`: a headless service on `rust-fx` alone (no HTTP, no purlin).
//!
//! Same layering as the web template, different edge: the "presentation" of a worker is its
//! schedule, so the application layer owns the use case (`schedule`) and the composition layer
//! wires ports to adapters with fx modules.

use super::{DependencySource, fill};
use crate::plan::Plan;

const CARGO: &str = r#"[package]
name = "__PACKAGE__"
version = "0.1.0"
edition = "2024"
publish = false

# No HTTP stack and no web framework: a worker needs dependency injection, lifecycle hooks and a
# graceful shutdown, which is exactly what rust-fx is.
[dependencies]
__FX__
tokio = { version = "1", features = ["macros", "rt-multi-thread", "time"] }

[dev-dependencies]
tokio = { version = "1", features = ["macros", "rt-multi-thread", "time", "test-util"] }
"#;

const LIB: &str = r#"//! __PACKAGE__, a worker in clean architecture on `rust-fx`.
//!
//! ```text
//! domain/          ports: what the worker needs from the world (Clock) and what it does (Job)
//! application/     the use case: run every job on a schedule              (-> domain)
//! infrastructure/  adapters implementing the ports                        (-> domain)
//! composition/     fx modules wiring ports to adapters, and `app()`       (-> everything)
//! ```
//!
//! Adding a job: implement `Job` in `infrastructure/jobs.rs` with `#[fx::bind(group)]`, give it
//! a constructor, and list it in `composition::JobsModule`. Nothing else changes.

pub mod application;
pub mod composition;
pub mod domain;
pub mod infrastructure;

pub use composition::app;
"#;

const MAIN: &str = r#"use std::sync::Arc;

use __LIB__::application::{Journal, Schedule};

#[tokio::main]
async fn main() -> fx::Result<()> {
    // Runs until Ctrl-C or SIGTERM, then stops hooks in reverse order.
    let code = __LIB__::app(Schedule::default(), Arc::new(Journal::default())).run().await?;
    std::process::exit(code);
}
"#;

const DOMAIN: &str = r#"//! **Domain layer**: ports. Depends on nothing but the `#[fx::port]` marker.

/// Time as a port, so tests can fix it.
#[fx::port]
pub trait Clock {
    /// Seconds since the Unix epoch.
    fn now_secs(&self) -> u64;
}

/// One unit of recurring work.
#[fx::port]
pub trait Job {
    /// A stable name, used in logs and the journal.
    fn name(&self) -> &'static str;

    /// Does the work and reports what happened.
    fn run(&self, now_secs: u64) -> String;
}
"#;

const APPLICATION: &str = r#"//! **Application layer**: the use case. Depends on the domain only.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use fx::{Group, Lifecycle, Shutdowner};

use crate::domain::{ClockPort, JobPort};

/// When and how often the jobs run.
#[derive(Debug, Clone)]
pub struct Schedule {
    /// Time between rounds.
    pub every: Duration,
    /// Stop after this many rounds; `None` runs until the process is asked to stop.
    pub rounds: Option<u32>,
}

impl Default for Schedule {
    fn default() -> Self {
        Self {
            every: Duration::from_secs(60),
            rounds: None,
        }
    }
}

/// What every job reported, in order. Also printed, so `cargo run` shows something.
#[derive(Default)]
pub struct Journal(Mutex<Vec<String>>);

impl Journal {
    /// Records one line.
    pub fn record(&self, line: String) {
        println!("{line}");
        self.0.lock().unwrap_or_else(PoisonError::into_inner).push(line);
    }

    /// Everything recorded so far.
    pub fn lines(&self) -> Vec<String> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

/// Runs every job each round. Registered as an `fx` invoke, so it starts with the app and its
/// task is cancelled by a stop hook when the app stops.
pub fn schedule(
    clock: Arc<ClockPort>,
    jobs: Group<JobPort>,
    plan: Arc<Schedule>,
    journal: Arc<Journal>,
    life: Lifecycle,
    stop: Shutdowner,
) {
    let handle = tokio::spawn(async move {
        let mut round = 0u32;
        loop {
            let now = clock.now_secs();
            for job in &jobs {
                journal.record(format!("{}: {}", job.name(), job.run(now)));
            }
            round += 1;
            if plan.rounds.is_some_and(|limit| round >= limit) {
                stop.shutdown_with(0);
                return;
            }
            tokio::time::sleep(plan.every).await;
        }
    });
    life.on_stop(move || async move {
        handle.abort();
    });
}
"#;

const INFRASTRUCTURE: &str = r#"//! **Infrastructure layer**: adapters implementing the domain's ports.

mod clock;
mod jobs;

pub use clock::SystemClock;
pub use jobs::Heartbeat;
"#;

const CLOCK: &str = r#"use std::time::{SystemTime, UNIX_EPOCH};

use crate::domain::Clock;

/// The real clock.
#[derive(Default)]
pub struct SystemClock;

#[fx::component]
impl SystemClock {
    /// Registered as a constructor; `#[fx::bind]` below also provides it as `ClockPort`.
    #[fx::constructor]
    pub fn new() -> Self {
        Self
    }
}

#[fx::bind]
impl Clock for SystemClock {
    fn now_secs(&self) -> u64 {
        SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
    }
}
"#;

const JOBS: &str = r#"use crate::domain::Job;

/// Proves the worker is alive. Replace it with real work.
#[derive(Default)]
pub struct Heartbeat;

#[fx::component]
impl Heartbeat {
    /// Registered as a constructor.
    #[fx::constructor]
    pub fn new() -> Self {
        Self
    }
}

// `group`: every job joins one group, which the schedule receives as `Group<JobPort>`.
#[fx::bind(group)]
impl Job for Heartbeat {
    fn name(&self) -> &'static str {
        "heartbeat"
    }

    fn run(&self, now_secs: u64) -> String {
        format!("alive at {now_secs}")
    }
}
"#;

const COMPOSITION: &str = r#"//! **Composition layer**: fx modules wire ports to adapters; `app()` assembles the program.

use std::sync::Arc;

use crate::application::{Journal, Schedule, schedule};
use crate::domain::ClockPort;
use crate::infrastructure::{Heartbeat, SystemClock};

/// Time. Exports only the port; the concrete clock stays private to the module.
#[fx::module(providers = [SystemClock], exports = [ClockPort])]
pub struct TimeModule;

/// The jobs. They join the `JobPort` group, which is app-wide.
#[fx::module(providers = [Heartbeat])]
pub struct JobsModule;

/// The whole program. Tests build it with their own schedule and journal, and can
/// `.replace::<ClockPort>(..)` any port.
pub fn app(plan: Schedule, journal: Arc<Journal>) -> fx::App {
    fx::App::new()
        .supply(plan)
        .supply_arc(journal)
        .module::<TimeModule>()
        .module::<JobsModule>()
        .invoke(schedule)
}
"#;

const SMOKE_TEST: &str = r#"use std::sync::Arc;
use std::time::Duration;

use __LIB__::application::{Journal, Schedule};
use __LIB__::domain::{Clock, ClockPort};

fn quick(rounds: u32) -> Schedule {
    Schedule {
        every: Duration::from_millis(1),
        rounds: Some(rounds),
    }
}

#[test]
fn the_wiring_is_valid() {
    __LIB__::app(Schedule::default(), Arc::default()).validate().unwrap();
}

#[tokio::test]
async fn it_runs_every_job_each_round_and_exits_cleanly() {
    let journal = Arc::new(Journal::default());
    let code = __LIB__::app(quick(2), journal.clone()).run().await.unwrap();
    assert_eq!(code, 0);
    assert_eq!(journal.lines().len(), 2);
    assert!(journal.lines().iter().all(|line| line.starts_with("heartbeat: alive at ")));
}

#[tokio::test]
async fn replacing_the_clock_needs_no_change_to_any_module() {
    struct Fixed;
    impl Clock for Fixed {
        fn now_secs(&self) -> u64 {
            42
        }
    }
    let journal = Arc::new(Journal::default());
    __LIB__::app(quick(1), journal.clone())
        .replace::<ClockPort>(Arc::new(Fixed))
        .run()
        .await
        .unwrap();
    assert_eq!(journal.lines(), ["heartbeat: alive at 42"]);
}
"#;

const GITIGNORE: &str = "/target\n";

/// The plan for a new worker project named `package` (library name `lib`).
pub fn worker(package: &str, lib: &str, source: &DependencySource) -> Plan {
    let tokens = [
        ("__PACKAGE__", package),
        ("__LIB__", lib),
        ("__FX__", &source.dep("rust-fx")),
    ];
    let mut plan = Plan::new();
    plan.create("Cargo.toml", fill(CARGO, &tokens))
        .create(".gitignore", GITIGNORE)
        .create("src/lib.rs", fill(LIB, &tokens))
        .create("src/main.rs", fill(MAIN, &tokens))
        .create("src/domain/mod.rs", DOMAIN)
        .create("src/application/mod.rs", APPLICATION)
        .create("src/infrastructure/mod.rs", INFRASTRUCTURE)
        .create("src/infrastructure/clock.rs", CLOCK)
        .create("src/infrastructure/jobs.rs", JOBS)
        .create("src/composition/mod.rs", COMPOSITION)
        .create("tests/smoke.rs", fill(SMOKE_TEST, &tokens));
    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contents(plan: &Plan, suffix: &str) -> String {
        plan.ops
            .iter()
            .find_map(|op| match op {
                crate::Op::Create { path, contents } if path.ends_with(suffix) => Some(contents.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no {suffix} in the plan"))
    }

    #[test]
    fn a_worker_depends_on_rust_fx_and_not_on_any_web_or_purlin_crate() {
        let plan = worker("my-worker", "my_worker", &DependencySource::Path("/src/purlin/".into()));
        let cargo = contents(&plan, "Cargo.toml");
        assert!(
            cargo.contains("rust-fx = { path = \"/src/purlin/crates/rust-fx\" }"),
            "{cargo}"
        );
        for forbidden in ["purlin ", "purlin-", "axum", "utoipa"] {
            assert!(!cargo.contains(forbidden), "{forbidden} in:\n{cargo}");
        }
        assert!(!cargo.contains("__"), "every token was replaced:\n{cargo}");
    }

    #[test]
    fn no_token_survives_in_any_generated_file() {
        let plan = worker("my-worker", "my_worker", &DependencySource::Version("0.1".into()));
        for op in &plan.ops {
            if let crate::Op::Create { path, contents } = op {
                assert!(
                    !contents.contains("__PACKAGE__") && !contents.contains("__LIB__"),
                    "{path:?}"
                );
            }
        }
        assert!(contents(&plan, "main.rs").contains("my_worker::app("));
    }
}
