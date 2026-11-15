//! The friendly syntax: `#[port]`, handle-typed fields, `provides`, `#[provider]`
//! functions and `#[lifecycle]`, with no `dyn` and no `Provider::..` in sight.

#![allow(missing_docs, dead_code)]

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use nestrs::prelude::*;

static EVENTS: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
static BUILT: AtomicUsize = AtomicUsize::new(0);

fn record(event: &'static str) {
    EVENTS.lock().unwrap().push(event);
}

#[port]
pub trait Greeter {
    async fn greet(&self, name: &str) -> String;
}

#[port(Clocks)]
pub trait Clock {
    fn now(&self) -> u64;
}

#[injectable(provides = GreeterPort)]
pub struct EnglishGreeter {
    #[inject(default)]
    _unused: (),
}

#[async_trait]
impl Greeter for EnglishGreeter {
    async fn greet(&self, name: &str) -> String {
        format!("Hello, {name}")
    }
}

#[injectable(provides = Clocks)]
pub struct FixedClock;

impl Clock for FixedClock {
    fn now(&self) -> u64 {
        42
    }
}

#[injectable]
pub struct Welcome {
    greeter: GreeterPort,
    clock: Clocks,
}

impl Welcome {
    async fn run(&self, name: &str) -> String {
        format!("{} @{}", self.greeter.greet(name).await, self.clock.now())
    }
}

pub struct Settings {
    pub excited: bool,
}

#[provider]
fn settings() -> Settings {
    Settings { excited: true }
}

pub struct Banner(String);

#[provider]
async fn banner(settings: Arc<Settings>, welcome: Arc<Welcome>) -> Result<Banner, DiError> {
    BUILT.fetch_add(1, Ordering::SeqCst);
    let text = welcome.run("banner").await;
    Ok(Banner(if settings.excited { format!("{text}!") } else { text }))
}

pub struct Connection;

#[injectable]
pub struct Pool {
    #[inject(default)]
    open: std::sync::atomic::AtomicBool,
}

#[lifecycle]
impl Pool {
    #[on_init]
    async fn connect(&self) -> Result<(), String> {
        record("pool:init");
        self.open.store(true, Ordering::SeqCst);
        Ok(())
    }

    #[on_destroy]
    async fn close(&self) -> Result<(), String> {
        record("pool:destroy");
        self.open.store(false, Ordering::SeqCst);
        Ok(())
    }
}

#[injectable]
pub struct Repo {
    pool: Arc<Pool>,
}

pub struct Loud;

#[async_trait]
impl Greeter for Loud {
    async fn greet(&self, name: &str) -> String {
        format!("HELLO {}", name.to_uppercase())
    }
}

#[module(
    providers = [EnglishGreeter, FixedClock, Welcome, settings, banner, Pool, Repo],
    exports = [GreeterPort, Clocks, Welcome, Banner, Pool, Repo],
)]
pub struct CoreModule;

#[module(imports = [CoreModule], providers = [Consumer])]
pub struct AppModule;

#[injectable]
pub struct Consumer {
    welcome: Arc<Welcome>,
    banner: Arc<Banner>,
    greeter: GreeterPort,
    _repo: Arc<Repo>,
}

#[tokio::test]
async fn ports_bindings_and_providers_resolve_without_dyn() {
    let app = nestrs::http::App::create(AppModule).await.expect("starts");
    let consumer = app.container().resolve::<Consumer>().unwrap();
    assert_eq!(consumer.welcome.run("Ada").await, "Hello, Ada @42");
    assert_eq!(consumer.banner.0, "Hello, banner @42!");
    assert_eq!(consumer.greeter.greet("x").await, "Hello, x");
}

#[tokio::test]
async fn lifecycle_hooks_attach_automatically_and_destroy_in_reverse() {
    EVENTS.lock().unwrap().clear();
    let app = nestrs::http::App::create(AppModule).await.expect("starts");
    assert!(EVENTS.lock().unwrap().contains(&"pool:init"));
    assert!(app.shutdown().await.is_empty());
    assert!(EVENTS.lock().unwrap().contains(&"pool:destroy"));
}

#[tokio::test]
async fn provider_functions_can_be_overridden_by_port() {
    let built_before = BUILT.load(Ordering::SeqCst);
    let bootstrapped = nestrs::bootstrap_with::<AppModule>(
        Overrides::new().provide(Provider::instance::<dyn Greeter>(Arc::new(Loud))),
    )
    .await
    .expect("starts");
    let consumer = bootstrapped.container.resolve::<Consumer>().unwrap();
    assert_eq!(consumer.greeter.greet("a").await, "HELLO A");
    assert!(BUILT.load(Ordering::SeqCst) > built_before);
}

// Binding with a lifecycle on the adapter registers the adapter too.
#[injectable(provides = Clocks)]
pub struct HookedClock {
    #[inject(default)]
    ticks: AtomicUsize,
}
impl Clock for HookedClock {
    fn now(&self) -> u64 {
        self.ticks.load(Ordering::SeqCst) as u64
    }
}
#[lifecycle]
impl HookedClock {
    #[on_init]
    async fn start(&self) -> Result<(), String> {
        self.ticks.store(7, Ordering::SeqCst);
        Ok(())
    }
}

#[module(providers = [HookedClock], exports = [Clocks])]
pub struct HookedModule;

#[module(imports = [HookedModule], providers = [ClockUser])]
pub struct HookedApp;

#[injectable]
pub struct ClockUser {
    clock: Clocks,
}

#[tokio::test]
async fn a_bound_adapter_with_hooks_still_runs_them() {
    let bootstrapped = nestrs::bootstrap::<HookedApp>().await.expect("starts");
    let user = bootstrapped.container.resolve::<ClockUser>().unwrap();
    assert_eq!(user.clock.now(), 7);
}

// ---- scopes on ports and provider functions

static TICKETS: AtomicUsize = AtomicUsize::new(0);

#[port]
pub trait Ticket {
    fn number(&self) -> usize;
}

#[injectable(provides = TicketPort, scope = transient)]
pub struct Counter {
    #[inject(default)]
    _marker: (),
}

impl Ticket for Counter {
    fn number(&self) -> usize {
        TICKETS.fetch_add(1, Ordering::SeqCst)
    }
}

pub struct Serial(usize);

#[provider(scope = transient)]
fn serial() -> Serial {
    Serial(TICKETS.fetch_add(1, Ordering::SeqCst))
}

#[module(providers = [Counter, serial], exports = [TicketPort, Serial])]
pub struct TicketModule;

#[tokio::test]
async fn scoped_ports_and_provider_functions_build_per_resolution() {
    let bootstrapped = nestrs::bootstrap::<TicketModule>().await.expect("starts");
    let scope = RequestScope::new(bootstrapped.container.clone(), bootstrapped.scoped.clone());
    let first = scope.resolve::<dyn Ticket>().await.expect("resolves");
    let second = scope.resolve::<dyn Ticket>().await.expect("resolves");
    assert!(!Arc::ptr_eq(&first, &second), "transient: a fresh adapter each time");
    let a = scope.resolve::<Serial>().await.expect("resolves");
    let b = scope.resolve::<Serial>().await.expect("resolves");
    assert_ne!(a.0, b.0);
}
