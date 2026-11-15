//! `nestrs generate module | job | queue-handler`.

use super::fill;
use crate::naming::Names;
use crate::plan::Plan;

const MODULE: &str = r#"use nestrs::prelude::*;

/// The __SNAKE__ module. Add providers, controllers and exports as the feature grows.
#[module]
pub struct __PASCAL__Module;
"#;

const JOB: &str = r#"use std::time::Duration;

use async_trait::async_trait;
use nestrs::prelude::*;
use nestrs_jobs::{Job, JobError, JobPort, Schedule};

/// Scheduled job `__KEBAB__`: runs every minute, on one instance at a time.
///
/// Change `schedule` (`Schedule::cron("0 0 * * * *")` is hourly) and put the work in `run`. It
/// should be idempotent: the lock bounds duplicates, it cannot promise exactly-once.
#[injectable(contributes = JobPort)]
pub struct __PASCAL__Job;

#[async_trait]
impl Job for __PASCAL__Job {
    fn name(&self) -> &str {
        "__KEBAB__"
    }

    fn schedule(&self) -> Schedule {
        Schedule::every(Duration::from_secs(60)).expect("a non-zero interval")
    }

    async fn run(&self) -> Result<(), JobError> {
        tracing::info!(job = "__KEBAB__", "running");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn it_runs_and_has_a_valid_schedule() {
        let job = __PASCAL__Job;
        assert_eq!(job.name(), "__KEBAB__");
        let _ = job.schedule(); // building it checks the interval or cron expression
        assert!(job.run().await.is_ok());
    }
}
"#;

const HANDLER: &str = r#"use async_trait::async_trait;
use nestrs::prelude::*;
use nestrs_jobs::{HandlerError, QueueHandler, QueueHandlerPort, decode};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The payload of the `__KEBAB__` queue. Producers enqueue it with
/// `queue.enqueue("__KEBAB__", &__PASCAL__Message { .. })`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct __PASCAL__Message {
    /// What to work on.
    pub id: Uuid,
}

/// Consumes the `__KEBAB__` queue. Delivery is at-least-once, so `handle` must be idempotent.
#[injectable(contributes = QueueHandlerPort)]
pub struct __PASCAL__Handler;

#[async_trait]
impl QueueHandler for __PASCAL__Handler {
    fn queue(&self) -> &str {
        "__KEBAB__"
    }

    async fn handle(&self, payload: &str) -> Result<(), HandlerError> {
        // A payload that does not decode can never succeed: `decode` marks it `Fatal`, which
        // buries it at once instead of retrying. Return `HandlerError::Retry` for transient failures.
        let message: __PASCAL__Message = decode(payload)?;
        tracing::info!(queue = "__KEBAB__", id = %message.id, "handled");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn it_handles_a_message_and_buries_garbage() {
        let handler = __PASCAL__Handler;
        let message = serde_json::to_string(&__PASCAL__Message { id: Uuid::new_v4() }).unwrap();
        assert_eq!(handler.handle(&message).await, Ok(()));
        assert!(matches!(handler.handle("not json").await, Err(HandlerError::Fatal(_))));
    }
}
"#;

fn tokens(names: &Names) -> [(&'static str, &str); 3] {
    [
        ("__PASCAL__", names.pascal.as_str()),
        ("__SNAKE__", names.snake.as_str()),
        ("__KEBAB__", names.route_singular.as_str()),
    ]
}

/// An empty feature module in `composition/`, imported by `AppModule`.
pub fn module(names: &Names) -> Plan {
    let (snake, pascal) = (&names.snake, &names.pascal);
    let mut plan = Plan::new();
    plan.create(
        format!("src/composition/{snake}_module.rs"),
        fill(MODULE, &tokens(names)),
    )
    .ensure_lines(
        "src/composition/mod.rs",
        [
            format!("mod {snake}_module;"),
            format!("pub use {snake}_module::{pascal}Module;"),
        ],
    )
    .import_module(
        "src/composition/app.rs",
        &format!("{pascal}Module"),
        &format!("super::{pascal}Module"),
    );
    plan
}

/// A scheduled job in `application/`, registered in `AppModule` together with `JobsModule`.
/// `jobs_dependency` is the `nestrs-jobs` line to add to `Cargo.toml`.
pub fn job(names: &Names, jobs_dependency: &str) -> Plan {
    let (snake, pascal) = (&names.snake, &names.pascal);
    let mut plan = Plan::new();
    plan.add_dependency("nestrs-jobs", jobs_dependency)
        .create(format!("src/application/{snake}_job.rs"), fill(JOB, &tokens(names)))
        .ensure_lines(
            "src/application/mod.rs",
            [
                format!("mod {snake}_job;"),
                format!("pub use {snake}_job::{pascal}Job;"),
            ],
        );
    register_in_app(&mut plan, &format!("{pascal}Job"));
    plan
}

/// A queue consumer and its message type in `application/`, registered like a job.
pub fn queue_handler(names: &Names, jobs_dependency: &str) -> Plan {
    let (snake, pascal) = (&names.snake, &names.pascal);
    let mut plan = Plan::new();
    plan.add_dependency("nestrs-jobs", jobs_dependency)
        .create(
            format!("src/application/{snake}_handler.rs"),
            fill(HANDLER, &tokens(names)),
        )
        .ensure_lines(
            "src/application/mod.rs",
            [
                format!("mod {snake}_handler;"),
                format!("pub use {snake}_handler::{{{pascal}Handler, {pascal}Message}};"),
            ],
        );
    register_in_app(&mut plan, &format!("{pascal}Handler"));
    plan
}

/// Adds `item` to the app's providers, and `JobsModule::in_memory()` to its imports once.
fn register_in_app(plan: &mut Plan, item: &str) {
    let app = "src/composition/app.rs";
    plan.patch_module(
        app,
        "imports",
        "JobsModule::in_memory()",
        "JobsModule",
        Some("nestrs_jobs::JobsModule"),
    )
    .patch_module(
        app,
        "providers",
        item,
        item,
        Some(&format!("crate::application::{item}")),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Op;

    #[test]
    fn no_placeholder_survives() {
        let names = Names::parse("send-welcome", None).unwrap();
        for plan in [module(&names), job(&names, "\"0.1\""), queue_handler(&names, "\"0.1\"")] {
            for op in plan.ops {
                if let Op::Create { path, contents } = op {
                    assert!(!contents.contains("__"), "{} has a placeholder", path.display());
                }
            }
        }
    }
}
