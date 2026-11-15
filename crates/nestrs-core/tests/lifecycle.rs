//! Init and destroy hooks: ordering, failure handling and idempotent shutdown.

#![allow(clippy::manual_async_fn)]

use std::future::Future;
use std::sync::{Arc, Mutex};

use nestrs_core::*;

type Log = Arc<Mutex<Vec<String>>>;

fn log_of(container: &Container) -> Log {
    container.resolve::<Mutex<Vec<String>>>().unwrap()
}

fn note(log: &Log, entry: &str) {
    log.lock().unwrap().push(entry.to_owned());
}

fn entries(log: &Log) -> Vec<String> {
    log.lock().unwrap().clone()
}

/// A provider that records its lifecycle; `Db` is built first, `Cache` depends on it.
struct Db {
    log: Log,
}
struct Cache {
    _db: Arc<Db>,
    log: Log,
}

impl Injectable for Db {
    fn dependencies() -> Vec<Token> {
        vec![Token::of::<Mutex<Vec<String>>>()]
    }
    fn build(c: &Container) -> impl Future<Output = Result<Self, DiError>> + Send {
        let log = log_of(c);
        async move { Ok(Self { log }) }
    }
}
impl Injectable for Cache {
    fn dependencies() -> Vec<Token> {
        vec![Token::of::<Db>(), Token::of::<Mutex<Vec<String>>>()]
    }
    fn build(c: &Container) -> impl Future<Output = Result<Self, DiError>> + Send {
        let result = c.resolve::<Db>().map(|db| Self {
            _db: db,
            log: log_of(c),
        });
        async move { result }
    }
}

impl OnModuleInit for Db {
    fn on_module_init(&self) -> impl Future<Output = Result<(), DiError>> + Send {
        note(&self.log, "init db");
        async { Ok(()) }
    }
}
impl OnModuleDestroy for Db {
    fn on_module_destroy(&self) -> impl Future<Output = Result<(), DiError>> + Send {
        note(&self.log, "destroy db");
        async { Ok(()) }
    }
}
impl OnModuleInit for Cache {
    fn on_module_init(&self) -> impl Future<Output = Result<(), DiError>> + Send {
        note(&self.log, "init cache");
        async { Ok(()) }
    }
}
impl OnModuleDestroy for Cache {
    fn on_module_destroy(&self) -> impl Future<Output = Result<(), DiError>> + Send {
        note(&self.log, "destroy cache");
        async { Ok(()) }
    }
}

fn log_provider() -> Provider {
    Provider::value(Mutex::new(Vec::<String>::new()))
}

struct AppModule;
impl Module for AppModule {
    fn definition() -> ModuleDef {
        ModuleDef::new("AppModule").providers([
            log_provider(),
            // Declared in the "wrong" order on purpose: dependencies still go first.
            Provider::of::<Cache>().with_init::<Cache>().with_destroy::<Cache>(),
            Provider::of::<Db>().with_init::<Db>().with_destroy::<Db>(),
        ])
    }
}

#[tokio::test]
async fn init_runs_dependencies_first_and_destroy_runs_in_reverse() {
    let booted = bootstrap::<AppModule>().await.unwrap();
    let log = log_of(&booted.container);
    assert_eq!(entries(&log), ["init db", "init cache"]);

    let errors = booted.shutdown.run(&booted.container).await;
    assert!(errors.is_empty());
    assert_eq!(
        entries(&log),
        ["init db", "init cache", "destroy cache", "destroy db"],
        "dependents are destroyed before what they depend on"
    );
}

#[tokio::test]
async fn shutdown_runs_only_once() {
    let booted = bootstrap::<AppModule>().await.unwrap();
    booted.shutdown.run(&booted.container).await;
    booted.shutdown.run(&booted.container).await;
    assert_eq!(
        entries(&log_of(&booted.container))
            .iter()
            .filter(|e| *e == "destroy db")
            .count(),
        1
    );
}

struct Failing;
impl OnModuleInit for Failing {
    fn on_module_init(&self) -> impl Future<Output = Result<(), DiError>> + Send {
        async { Err(DiError::custom("disk full")) }
    }
}
impl Injectable for Failing {
    fn dependencies() -> Vec<Token> {
        vec![Token::of::<Db>()]
    }
    fn build(_: &Container) -> impl Future<Output = Result<Self, DiError>> + Send {
        async { Ok(Self) }
    }
}

struct FailingModule;
impl Module for FailingModule {
    fn definition() -> ModuleDef {
        ModuleDef::new("FailingModule").providers([
            log_provider(),
            Provider::of::<Db>().with_init::<Db>().with_destroy::<Db>(),
            Provider::of::<Failing>().with_init::<Failing>(),
        ])
    }
}

#[tokio::test]
async fn a_failing_init_aborts_startup_and_tears_down_what_had_started() {
    // The log lives in the container, which bootstrap drops on failure, so share
    // it through an override to observe the teardown.
    let log: Log = Arc::default();
    let error = bootstrap_with::<FailingModule>(Overrides::new().provide(Provider::instance(log.clone())))
        .await
        .unwrap_err();

    match error {
        DiError::Hook { phase, message, .. } => {
            assert_eq!(phase, "init");
            assert!(message.contains("disk full"), "{message}");
        }
        other => panic!("expected a hook error, got {other}"),
    }
    assert_eq!(entries(&log), ["init db", "destroy db"]);
}

struct Flaky;
impl Injectable for Flaky {
    fn dependencies() -> Vec<Token> {
        vec![]
    }
    fn build(_: &Container) -> impl Future<Output = Result<Self, DiError>> + Send {
        async { Ok(Self) }
    }
}

struct DestroyErrors;
impl Module for DestroyErrors {
    fn definition() -> ModuleDef {
        ModuleDef::new("DestroyErrors").providers([
            log_provider(),
            Provider::of::<Flaky>().on_destroy::<Flaky>(|_| Box::pin(async { Err(DiError::custom("boom")) })),
            Provider::of::<Db>().with_destroy::<Db>(),
        ])
    }
}

#[tokio::test]
async fn one_failing_destroy_hook_does_not_stop_the_others() {
    let booted = bootstrap::<DestroyErrors>().await.unwrap();
    let errors = booted.shutdown.run(&booted.container).await;
    assert_eq!(errors.len(), 1);
    assert!(
        matches!(&errors[0], DiError::Hook { phase: "destroy", .. }),
        "{}",
        errors[0]
    );
    assert_eq!(entries(&log_of(&booted.container)), ["destroy db"]);
}

trait Greeter: Send + Sync {}
struct English;
impl Greeter for English {}
impl Injectable for English {
    fn dependencies() -> Vec<Token> {
        vec![Token::of::<Mutex<Vec<String>>>()]
    }
    fn build(_: &Container) -> impl Future<Output = Result<Self, DiError>> + Send {
        async { Ok(Self) }
    }
}

struct BoundModule;
impl Module for BoundModule {
    fn definition() -> ModuleDef {
        ModuleDef::new("BoundModule").providers([
            log_provider(),
            bind!(dyn Greeter => English).on_init::<dyn Greeter>(|_| Box::pin(async { Ok(()) })),
        ])
    }
}

#[tokio::test]
async fn hooks_work_for_bound_interfaces() {
    assert!(bootstrap::<BoundModule>().await.is_ok());
}
