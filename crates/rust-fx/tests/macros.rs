#![allow(missing_docs, clippy::new_without_default)]
//! The macro surface: `#[fx::component]`, `#[fx::module]`, `fx::Params`, `fx::Component`.

use std::sync::{Arc, Mutex};

use fx::{App, Error, Group, Module};

// ---- the shape from the design discussion ----------------------------------------

#[allow(missing_docs)]
pub struct Clock;

#[fx::component]
impl Clock {
    #[fx::provide]
    pub fn new() -> Self {
        Self
    }
}

#[fx::module(
    providers = [Clock::new],
    exports = [Clock]
)]
pub struct ClockModule;

#[tokio::test]
async fn the_minimal_component_and_module_work_as_written() {
    let mut app = App::new().module::<ClockModule>();
    app.start().await.unwrap();
    assert!(app.get::<Clock>().await.is_ok());
}

#[tokio::test]
async fn listing_the_component_type_registers_the_same_thing() {
    #[fx::module(providers = [Clock], exports = [Clock])]
    struct ByType;
    let mut app = App::new().module::<ByType>();
    app.start().await.unwrap();
    assert!(app.get::<Clock>().await.is_ok());
}

// ---- components with dependencies, hooks and interfaces ----------------------------

type Log = Arc<Mutex<Vec<String>>>;

struct Config {
    url: &'static str,
}

#[derive(fx::Component)]
struct Marker;

fn marker() -> Marker {
    Marker
}

#[fx::component]
impl Config {
    #[fx::provide]
    fn load() -> Self {
        Self { url: "postgres://x" }
    }
}

trait Repo: Send + Sync {
    fn url(&self) -> String;
}

struct SqlRepo {
    config: Arc<Config>,
    log: Log,
}

#[fx::component]
impl SqlRepo {
    #[fx::provide(as = dyn Repo)]
    async fn connect(config: Arc<Config>, log: Arc<Mutex<Vec<String>>>) -> Result<Self, std::io::Error> {
        Ok(Self { config, log })
    }

    #[fx::on_start]
    async fn migrate(&self) -> Result<(), std::io::Error> {
        self.log.lock().unwrap().push(format!("migrate {}", self.config.url));
        Ok(())
    }

    #[fx::on_stop]
    fn close(&self) {
        self.log.lock().unwrap().push("close".into());
    }
}

impl Repo for SqlRepo {
    fn url(&self) -> String {
        self.config.url.to_string()
    }
}

#[fx::module(
    name = "data",
    providers = [Config, SqlRepo],
    exports = [dyn Repo],
)]
struct DataModule;

#[tokio::test]
async fn components_bring_their_hooks_and_interface_bindings() {
    let log: Log = Arc::default();
    let seen = Arc::new(Mutex::new(String::new()));
    let s = seen.clone();
    let mut app = App::new()
        .supply_arc::<Mutex<Vec<String>>>(log.clone())
        .module::<DataModule>()
        .invoke(move |repo: Arc<dyn Repo>| *s.lock().unwrap() = repo.url());

    app.start().await.unwrap();
    assert_eq!(*seen.lock().unwrap(), "postgres://x");
    assert_eq!(*log.lock().unwrap(), ["migrate postgres://x"]);

    app.stop().await.unwrap();
    assert_eq!(*log.lock().unwrap(), ["migrate postgres://x", "close"]);
}

#[test]
fn the_concrete_type_stays_private_when_bound_to_an_interface() {
    let error = App::new()
        .supply_arc::<Mutex<Vec<String>>>(Arc::default())
        .module::<DataModule>()
        .invoke(|_: Arc<SqlRepo>| {})
        .validate()
        .unwrap_err();
    assert!(matches!(error, Error::Missing { .. }), "{error}");
}

#[test]
fn derive_component_allows_returning_a_type_by_value() {
    App::new()
        .provide(marker)
        .invoke(|_: Arc<Marker>| {})
        .validate()
        .unwrap();
}

// ---- names, groups, invokes, decorators inside a component ---------------------------

trait Job: Send + Sync {
    fn id(&self) -> &'static str;
}

struct Jobs;
#[derive(fx::Component)]
struct Alpha;
#[derive(fx::Component)]
struct Beta;
impl Job for Alpha {
    fn id(&self) -> &'static str {
        "alpha"
    }
}
impl Job for Beta {
    fn id(&self) -> &'static str {
        "beta"
    }
}

fx::tag!(Primary = "primary");

#[fx::component]
impl Jobs {
    #[fx::provide(as = dyn Job, group)]
    fn alpha() -> Alpha {
        Alpha
    }

    #[fx::provide(as = dyn Job, group)]
    fn beta() -> Beta {
        Beta
    }

    #[fx::provide(name = "primary")]
    fn primary() -> Arc<String> {
        Arc::new("p".to_string())
    }

    #[fx::provide]
    fn plain() -> Arc<u32> {
        Arc::new(1)
    }

    #[fx::decorate]
    fn double(n: Arc<u32>) -> Arc<u32> {
        Arc::new(*n * 2)
    }

    #[fx::invoke]
    fn announce(jobs: Group<dyn Job>, primary: fx::Named<Primary, String>, n: Arc<u32>) -> Result<(), String> {
        let ids: Vec<_> = jobs.iter().map(|j| j.id()).collect();
        if ids == ["alpha", "beta"] && **primary == *"p" && *n == 2 {
            Ok(())
        } else {
            Err(format!("unexpected {ids:?} {} {n}", &**primary))
        }
    }
}

#[tokio::test]
async fn a_component_can_declare_groups_names_decorators_and_invokes() {
    // `announce` fails startup unless group order, the name and the decorator all worked.
    App::new().component::<Jobs>().run_until_started().await.unwrap();
}

// ---- Params ----------------------------------------------------------------------------

#[derive(fx::Params)]
struct Deps {
    config: Arc<Config>,
    jobs: Group<dyn Job>,
    cache: Option<Arc<String>>,
    life: fx::Lifecycle,
}

#[derive(fx::Params)]
struct Pair(Arc<Config>, Option<Arc<u8>>);

#[tokio::test]
async fn params_bundle_several_extractors() {
    let seen = Arc::new(Mutex::new((String::new(), 0usize, true, false)));
    let s = seen.clone();
    App::new()
        .provide(Config::load)
        .provide(fx::bind!(dyn Job, || Arc::new(Alpha)).in_group())
        .invoke(move |deps: Deps, pair: Pair| {
            *s.lock().unwrap() = (
                deps.config.url.to_string(),
                deps.jobs.len(),
                deps.cache.is_some(),
                pair.1.is_some(),
            );
            let _ = (&deps.life, &pair.0);
        })
        .run_until_started()
        .await
        .unwrap();
    assert_eq!(*seen.lock().unwrap(), ("postgres://x".to_string(), 1, false, false));
}

// ---- modules ---------------------------------------------------------------------------

struct Greeter(String);

#[fx::component]
impl Greeter {
    #[fx::provide]
    fn new(clock: Arc<Clock>) -> Self {
        let _ = clock;
        Self("hi".into())
    }
}

#[fx::module(imports = [ClockModule], providers = [Greeter], exports = [Greeter])]
struct GreetingModule;

fn say(greeter: Arc<Greeter>, log: Arc<Mutex<Vec<String>>>) {
    log.lock().unwrap().push(greeter.0.clone());
}

#[fx::module(imports = [GreetingModule], invokes = [say])]
struct AppModule;

#[tokio::test]
async fn imports_exports_and_invokes_compose() {
    let log: Log = Arc::default();
    App::new()
        .supply_arc::<Mutex<Vec<String>>>(log.clone())
        .module::<AppModule>()
        .run_until_started()
        .await
        .unwrap();
    assert_eq!(*log.lock().unwrap(), ["hi"]);
}

#[test]
fn a_module_that_does_not_import_what_it_uses_fails_validation_with_a_helpful_message() {
    #[fx::module(providers = [Greeter])]
    struct Lonely;
    let error = App::new().module::<Lonely>().validate().unwrap_err();
    assert!(error.to_string().contains("Clock"), "{error}");
}

#[tokio::test]
async fn module_options_and_named_exports() {
    struct Db;
    #[fx::component]
    impl Db {
        #[fx::provide(name = "primary")]
        fn primary() -> Self {
            Db
        }
    }
    #[fx::module(providers = [Db], exports = [Db as "primary"])]
    struct Named;
    let mut app = App::new().options(Named::options());
    app.start().await.unwrap();
    assert!(app.get_named::<Db>("primary").await.is_ok());
}
