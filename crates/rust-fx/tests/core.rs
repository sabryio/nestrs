#![allow(missing_docs)]
//! Behaviour of the runtime, written against the function API only (no macros).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use fx::{App, Error, Group, Lifecycle, Named, Options, Phase, Shutdowner};

type Log = Arc<Mutex<Vec<String>>>;

fn log() -> Log {
    Arc::default()
}

fn push(log: &Log, line: impl Into<String>) {
    log.lock().unwrap().push(line.into());
}

fn lines(log: &Log) -> Vec<String> {
    log.lock().unwrap().clone()
}

#[derive(Debug)]
struct Config(&'static str);
struct Database(Arc<Config>);
#[allow(dead_code)]
struct Repo(Arc<Database>);
#[allow(dead_code)]
struct Service(Arc<Repo>);

fn config() -> Arc<Config> {
    Arc::new(Config("url"))
}

fn database(config: Arc<Config>) -> Arc<Database> {
    Arc::new(Database(config))
}

fn repo(db: Arc<Database>) -> Arc<Repo> {
    Arc::new(Repo(db))
}

// ---- construction and order -----------------------------------------------------

#[tokio::test]
async fn constructors_run_dependencies_first_and_invokes_in_order() {
    let log = log();
    let (l1, l2, l3, l4) = (log.clone(), log.clone(), log.clone(), log.clone());

    let mut app = App::new()
        .constructor(move |db: Arc<Database>| {
            push(&l1, "repo");
            Arc::new(Repo(db))
        })
        .constructor(move |cfg: Arc<Config>| {
            push(&l2, "database");
            Arc::new(Database(cfg))
        })
        .constructor(move || {
            push(&l3, "config");
            Arc::new(Config("x"))
        })
        .invoke(move |_: Arc<Repo>| push(&l4, "invoke-1"))
        .invoke({
            let log = log.clone();
            move |_: Arc<Config>| push(&log, "invoke-2")
        });

    app.start().await.unwrap();
    assert_eq!(lines(&log), ["config", "database", "repo", "invoke-1", "invoke-2"]);
    app.stop().await.unwrap();
}

#[tokio::test]
async fn only_what_an_invoke_needs_is_built() {
    let log = log();
    let unused = log.clone();
    let mut app = App::new()
        .constructor(config)
        .constructor(move || {
            push(&unused, "unused built");
            Arc::new(Service(Arc::new(Repo(Arc::new(Database(Arc::new(Config(""))))))))
        })
        .invoke(|_: Arc<Config>| {});
    app.start().await.unwrap();
    assert!(lines(&log).is_empty(), "providers nobody asks for stay unbuilt");

    // ...but they can still be fetched on demand.
    let _service = app.get::<Service>().await.unwrap();
    assert_eq!(lines(&log), ["unused built"]);
    app.stop().await.unwrap();
}

#[tokio::test]
async fn async_and_fallible_constructors_work() {
    async fn slow_config() -> Result<Arc<Config>, std::io::Error> {
        tokio::task::yield_now().await;
        Ok(Arc::new(Config("async")))
    }
    let mut app = App::new().constructor(slow_config);
    app.validate().unwrap();
    app.start().await.unwrap();
    assert_eq!(app.get::<Config>().await.unwrap().0, "async");
    app.stop().await.unwrap();
}

#[tokio::test]
async fn a_failing_constructor_stops_startup_and_names_the_provider() {
    fn broken() -> Result<Arc<Config>, std::io::Error> {
        Err(std::io::Error::other("disk on fire"))
    }
    let mut app = App::new().constructor(broken).invoke(|_: Arc<Config>| {});
    let error = app.start().await.unwrap_err();
    assert!(matches!(error, Error::Build { .. }), "{error}");
    let text = error.to_string();
    assert!(
        text.contains("broken") && text.contains("disk on fire") && text.contains("Config"),
        "{text}"
    );
    assert!(!app.is_running());
}

#[tokio::test]
async fn a_failing_invoke_is_reported_with_its_origin() {
    let mut app = App::new().invoke(|| -> Result<(), String> { Err("nope".into()) });
    let error = app.start().await.unwrap_err();
    assert!(matches!(error, Error::Invoke { .. }), "{error}");
    assert!(error.to_string().contains("nope"));
}

#[tokio::test]
async fn supply_provides_a_value_and_supply_arc_a_trait_object() {
    trait Clock: Send + Sync {
        fn now(&self) -> u64;
    }
    struct Fixed;
    impl Clock for Fixed {
        fn now(&self) -> u64 {
            42
        }
    }
    let mut app = App::new()
        .supply(Config("supplied"))
        .supply_arc::<dyn Clock>(Arc::new(Fixed));
    app.start().await.unwrap();
    assert_eq!(app.get::<Config>().await.unwrap().0, "supplied");
    assert_eq!(app.get::<dyn Clock>().await.unwrap().now(), 42);
}

// ---- validation -----------------------------------------------------------------

#[test]
fn a_missing_dependency_reports_the_chain() {
    let app = App::new()
        .constructor(repo)
        .constructor(|r: Arc<Repo>| Arc::new(Service(r)))
        .invoke(|_: Arc<Service>| {});
    let error = app.validate().unwrap_err();
    match &error {
        Error::Missing { key, .. } => assert!(key.to_string().contains("Database")),
        other => panic!("unexpected {other}"),
    }
    let text = error.to_string();
    assert!(text.contains("needs `Database`"), "{text}");
    assert!(text.contains("required via"), "{text}");
}

#[test]
fn unused_providers_are_validated_too() {
    let error = App::new().constructor(repo).validate().unwrap_err();
    assert!(matches!(error, Error::Missing { .. }));
}

#[test]
fn a_cycle_is_reported_with_the_full_loop() {
    struct A;
    struct B;
    let app = App::new()
        .constructor(|_: Arc<B>| Arc::new(A))
        .constructor(|_: Arc<A>| Arc::new(B))
        .invoke(|_: Arc<A>| {});
    match app.validate().unwrap_err() {
        Error::Cycle { path } => {
            assert_eq!(path.first(), path.last());
            assert!(path.len() >= 3, "{path:?}");
        }
        other => panic!("unexpected {other}"),
    }
}

#[test]
fn a_duplicate_provider_in_one_scope_is_rejected() {
    let error = App::new()
        .constructor(config)
        .constructor(config)
        .validate()
        .unwrap_err();
    assert!(matches!(error, Error::Duplicate { .. }), "{error}");
}

#[test]
fn a_decorator_must_take_what_it_decorates() {
    let error = App::new()
        .constructor(config)
        .decorate(|| Arc::new(Config("x")))
        .validate()
        .unwrap_err();
    assert!(matches!(error, Error::DecoratorInput { .. }), "{error}");
}

#[test]
fn a_decorator_for_something_unprovided_is_a_missing_dependency() {
    let error = App::new().decorate(|c: Arc<Config>| c).validate().unwrap_err();
    assert!(matches!(error, Error::Missing { .. }), "{error}");
}

#[tokio::test]
async fn validation_does_not_run_constructors() {
    let log = log();
    let l = log.clone();
    let app = App::new().constructor(move || {
        push(&l, "built");
        Arc::new(Config(""))
    });
    app.validate().unwrap();
    assert!(lines(&log).is_empty());
}

// ---- extractors -----------------------------------------------------------------

#[tokio::test]
async fn optional_is_none_when_unprovided_and_some_when_provided() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s = seen.clone();
    let probe = move |c: Option<Arc<Config>>| s.lock().unwrap().push(c.is_some());

    App::new().invoke(probe.clone()).run_until_started().await.unwrap();
    App::new()
        .constructor(config)
        .invoke(probe)
        .run_until_started()
        .await
        .unwrap();
    assert_eq!(*seen.lock().unwrap(), [false, true]);
}

fx::tag!(Primary = "primary");
fx::tag!(Replica = "replica");

#[tokio::test]
async fn named_values_are_kept_apart() {
    let seen = Arc::new(Mutex::new(String::new()));
    let s = seen.clone();
    let mut app = App::new()
        .constructor(fx::constructor(|| Arc::new(Config("p"))).named("primary"))
        .constructor(fx::constructor(|| Arc::new(Config("r"))).named("replica"))
        .constructor(config)
        .invoke(
            move |p: Named<Primary, Config>, r: Named<Replica, Config>, plain: Arc<Config>| {
                *s.lock().unwrap() = format!("{}{}{}", p.0, r.0, plain.0);
            },
        );
    app.start().await.unwrap();
    assert_eq!(*seen.lock().unwrap(), "prurl");
    assert_eq!(app.get_named::<Config>("replica").await.unwrap().0, "r");
}

trait Job: Send + Sync {
    fn name(&self) -> &'static str;
}
struct Cleanup;
struct Report;
#[fx::bind(group)]
impl Job for Cleanup {
    fn name(&self) -> &'static str {
        "cleanup"
    }
}
#[fx::bind(group)]
impl Job for Report {
    fn name(&self) -> &'static str {
        "report"
    }
}

struct Solo;
#[fx::bind]
impl Job for Solo {
    fn name(&self) -> &'static str {
        "solo"
    }
}

fx::tag!(Nightly = "nightly");

#[tokio::test]
async fn groups_collect_every_contribution_in_registration_order() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (s1, s2) = (seen.clone(), seen.clone());
    App::new()
        .constructor(|| Arc::new(Cleanup))
        .constructor(|| Arc::new(Report))
        // `#[fx::bind(group)]` already contributed both to the unnamed group; the named
        // group is a placement this program chooses itself.
        .constructor(fx::bind::<dyn Job, Report>().in_group_named("nightly"))
        .invoke(move |jobs: Group<dyn Job>| {
            s1.lock().unwrap().extend(jobs.iter().map(|j| j.name().to_string()));
        })
        .invoke(move |jobs: Group<dyn Job, Nightly>| {
            s2.lock()
                .unwrap()
                .extend(jobs.iter().map(|j| format!("nightly:{}", j.name())));
        })
        .run_until_started()
        .await
        .unwrap();
    assert_eq!(*seen.lock().unwrap(), ["cleanup", "report", "nightly:report"]);
}

#[tokio::test]
async fn an_empty_group_is_fine() {
    let seen = Arc::new(Mutex::new(usize::MAX));
    let s = seen.clone();
    App::new()
        .invoke(move |jobs: Group<dyn Job>| *s.lock().unwrap() = jobs.len())
        .run_until_started()
        .await
        .unwrap();
    assert_eq!(*seen.lock().unwrap(), 0);
}

#[tokio::test]
async fn provide_bind_swaps_the_concrete_type_for_the_interface() {
    let mut app = App::new().constructor(fx::constructor(|| Arc::new(Cleanup)).bind::<dyn Job>());
    app.start().await.unwrap();
    assert_eq!(app.get::<dyn Job>().await.unwrap().name(), "cleanup");
    assert!(matches!(app.get::<Cleanup>().await, Err(Error::Missing { .. })));
}

#[tokio::test]
async fn bind_declarations_follow_the_constructor_of_the_type_automatically() {
    // `#[fx::bind] impl Job for Cleanup` is all it takes: no `as`, no cast, no `dyn` here.
    let mut app = App::new().constructor(|| Arc::new(Solo));
    app.start().await.unwrap();
    assert_eq!(app.get::<dyn Job>().await.unwrap().name(), "solo");
    assert_eq!(
        app.get::<Solo>().await.unwrap().name(),
        "solo",
        "the concrete type stays"
    );
}

#[tokio::test]
async fn bind_declarations_do_not_apply_to_named_or_grouped_values() {
    let mut app = App::new().constructor(fx::constructor(|| Arc::new(Cleanup)).named("x"));
    app.start().await.unwrap();
    assert!(matches!(app.get::<dyn Job>().await, Err(Error::Missing { .. })));
}

// ---- decorators -----------------------------------------------------------------

#[tokio::test]
async fn a_root_decorator_wraps_the_value_for_every_consumer() {
    let mut app = App::new()
        .constructor(config)
        .decorate(|c: Arc<Config>, _: Arc<Config>| Arc::new(Config(if c.0 == "url" { "decorated" } else { "?" })))
        .invoke(|_: Arc<Config>| {});
    // Two parameters of the decorated type both bind to the undecorated value.
    app.start().await.unwrap();
    assert_eq!(app.get::<Config>().await.unwrap().0, "decorated");
}

#[tokio::test]
async fn decorators_chain_in_registration_order_and_can_take_other_dependencies() {
    struct Suffix(&'static str);
    let mut app = App::new()
        .supply(Suffix("!"))
        .constructor(|| Arc::new(String::from("hi")))
        .decorate(|s: Arc<String>| Arc::new(format!("{s}1")))
        .decorate(|s: Arc<String>, suffix: Arc<Suffix>| Arc::new(format!("{s}2{}", suffix.0)));
    app.start().await.unwrap();
    assert_eq!(*app.get::<String>().await.unwrap(), "hi12!");
}

// ---- modules --------------------------------------------------------------------

use fx::{Export, Module, ModuleDef, ModuleRef};

struct DataModule;
impl Module for DataModule {
    fn definition() -> ModuleDef {
        ModuleDef::new("data")
            .options(
                Options::new()
                    .constructor(config)
                    .constructor(database)
                    .constructor(repo),
            )
            .export(Export::of::<Repo>())
    }
}

struct FeatureModule;
impl Module for FeatureModule {
    fn definition() -> ModuleDef {
        ModuleDef::new("feature")
            .import(ModuleRef::of::<DataModule>())
            .options(Options::new().constructor(|r: Arc<Repo>| Arc::new(Service(r))))
            .export(Export::of::<Service>())
    }
}

#[tokio::test]
async fn modules_expose_only_their_exports() {
    let mut app = App::new().module::<FeatureModule>().invoke(|_: Arc<Service>| {});
    app.start().await.unwrap();
    assert!(app.get::<Service>().await.is_ok());
    // The repo is exported by `data`, but `data` is not imported at the root.
    assert!(matches!(app.get::<Repo>().await, Err(Error::Missing { .. })));
    // Database is private to `data`.
    assert!(matches!(app.get::<Database>().await, Err(Error::Missing { .. })));
}

#[test]
fn reaching_into_a_modules_private_provider_says_so() {
    let error = App::new()
        .module::<DataModule>()
        .invoke(|_: Arc<Database>| {})
        .validate()
        .unwrap_err();
    match &error {
        Error::NotVisible { provided_in, .. } => assert_eq!(&**provided_in, "data"),
        other => panic!("unexpected {other}"),
    }
    assert!(error.to_string().contains("does not export"));
}

#[test]
fn the_same_module_imported_twice_is_one_scope() {
    let app = App::new()
        .module::<DataModule>()
        .module::<FeatureModule>()
        .invoke(|_: Arc<Repo>, _: Arc<Service>| {});
    app.validate().unwrap();
}

#[test]
fn exporting_something_you_do_not_have_is_an_error() {
    struct Bad;
    impl Module for Bad {
        fn definition() -> ModuleDef {
            ModuleDef::new("bad").export(Export::of::<Config>())
        }
    }
    assert!(matches!(
        App::new().module::<Bad>().validate(),
        Err(Error::InvalidExport { .. })
    ));
}

#[test]
fn import_cycles_are_reported_instead_of_recursing() {
    struct A;
    struct B;
    impl Module for A {
        fn definition() -> ModuleDef {
            ModuleDef::new("a").import(ModuleRef::of::<B>())
        }
    }
    impl Module for B {
        fn definition() -> ModuleDef {
            ModuleDef::new("b").import(ModuleRef::of::<A>())
        }
    }
    match App::new().module::<A>().validate().unwrap_err() {
        Error::ModuleCycle { path } => assert_eq!(path.len(), 3),
        other => panic!("unexpected {other}"),
    }
}

#[test]
fn two_imports_exporting_the_same_key_are_ambiguous() {
    struct One;
    struct Two;
    struct Both;
    impl Module for One {
        fn definition() -> ModuleDef {
            ModuleDef::new("one")
                .options(Options::new().constructor(config))
                .export(Export::of::<Config>())
        }
    }
    impl Module for Two {
        fn definition() -> ModuleDef {
            ModuleDef::new("two")
                .options(Options::new().constructor(config))
                .export(Export::of::<Config>())
        }
    }
    impl Module for Both {
        fn definition() -> ModuleDef {
            ModuleDef::new("both")
                .import(ModuleRef::of::<One>())
                .import(ModuleRef::of::<Two>())
                .options(Options::new().invoke(|_: Arc<Config>| {}))
        }
    }
    assert!(matches!(
        App::new().module::<Both>().validate(),
        Err(Error::Ambiguous { .. })
    ));
}

#[tokio::test]
async fn a_module_provider_shadows_the_root_and_a_module_decorator_stays_inside() {
    struct Inner;
    impl Module for Inner {
        fn definition() -> ModuleDef {
            ModuleDef::new("inner").options(
                Options::new()
                    .constructor(|| Arc::new(String::from("inner")))
                    .invoke(|s: Arc<String>| assert_eq!(*s, "inner+"))
                    .decorate(|s: Arc<String>| Arc::new(format!("{s}+"))),
            )
        }
    }
    let mut app = App::new()
        .constructor(|| Arc::new(String::from("root")))
        .module::<Inner>();
    app.start().await.unwrap();
    // The inner decorator did not touch the root's value.
    assert_eq!(*app.get::<String>().await.unwrap(), "root");
}

// ---- lifecycle ------------------------------------------------------------------

fn tracked(log: &Log, name: &'static str, life: &Lifecycle) {
    let (a, b) = (log.clone(), log.clone());
    life.append(
        fx::Hook::new()
            .on_start(move || async move { push(&a, format!("start {name}")) })
            .on_stop(move || async move { push(&b, format!("stop {name}")) }),
    );
}

#[tokio::test]
async fn hooks_start_in_order_and_stop_in_reverse() {
    let log = log();
    let (l1, l2) = (log.clone(), log.clone());
    let mut app = App::new()
        .constructor(move |life: Lifecycle| {
            tracked(&l1, "config", &life);
            Arc::new(Config(""))
        })
        .constructor(move |_: Arc<Config>, life: Lifecycle| {
            tracked(&l2, "database", &life);
            Arc::new(Database(Arc::new(Config(""))))
        })
        .invoke(|_: Arc<Database>| {});
    app.start().await.unwrap();
    assert_eq!(lines(&log), ["start config", "start database"]);
    app.stop().await.unwrap();
    assert_eq!(
        lines(&log),
        ["start config", "start database", "stop database", "stop config"]
    );
    assert!(matches!(app.stop().await, Err(Error::NotStarted)));
}

#[tokio::test]
async fn provide_builders_attach_hooks_to_the_built_value() {
    let log = log();
    let (a, b) = (log.clone(), log.clone());
    let mut app = App::new()
        .constructor(
            fx::constructor(config)
                .on_start(move |c: Arc<Config>| {
                    let a = a.clone();
                    async move { push(&a, format!("up {}", c.0)) }
                })
                .on_stop(move |c: Arc<Config>| {
                    let b = b.clone();
                    async move { push(&b, format!("down {}", c.0)) }
                }),
        )
        .invoke(|_: Arc<Config>| {});
    app.start().await.unwrap();
    app.stop().await.unwrap();
    assert_eq!(lines(&log), ["up url", "down url"]);
}

#[tokio::test]
async fn a_failing_start_hook_rolls_back_the_ones_that_started() {
    let log = log();
    let (l1, l2) = (log.clone(), log.clone());
    let mut app = App::new()
        .constructor(move |life: Lifecycle| {
            tracked(&l1, "first", &life);
            Arc::new(Config(""))
        })
        .constructor(move |_: Arc<Config>, life: Lifecycle| {
            let l = l2.clone();
            life.append(
                fx::Hook::new()
                    .on_start(|| async { Err::<(), _>("boom") })
                    .on_stop(move || async move { push(&l, "stop second") }),
            );
            Arc::new(Database(Arc::new(Config(""))))
        })
        .invoke(|_: Arc<Database>| {});
    let error = app.start().await.unwrap_err();
    assert!(
        matches!(
            error,
            Error::Hook {
                phase: Phase::Start,
                ..
            }
        ),
        "{error}"
    );
    assert!(error.to_string().contains("boom"));
    // `first` started, so it is stopped; `second` never started, so it is not.
    assert_eq!(lines(&log), ["start first", "stop first"]);
    assert!(!app.is_running());
}

#[tokio::test]
async fn every_stop_hook_runs_even_when_one_fails() {
    let log = log();
    let (l1, l2) = (log.clone(), log.clone());
    let mut app = App::new()
        .constructor(move |life: Lifecycle| {
            life.on_stop({
                let l = l1.clone();
                move || async move { push(&l, "stop a") }
            });
            Arc::new(Config(""))
        })
        .constructor(move |_: Arc<Config>, life: Lifecycle| {
            life.on_stop(|| async { Err::<(), _>("b failed") });
            life.on_stop({
                let l = l2.clone();
                move || async move { push(&l, "stop c") }
            });
            Arc::new(Database(Arc::new(Config(""))))
        })
        .invoke(|_: Arc<Database>| {});
    app.start().await.unwrap();
    let error = app.stop().await.unwrap_err();
    match &error {
        Error::Stop { errors } => assert_eq!(errors.len(), 1),
        other => panic!("unexpected {other}"),
    }
    assert!(error.to_string().contains("b failed"));
    assert_eq!(lines(&log), ["stop c", "stop a"]);
}

#[tokio::test]
async fn starting_twice_is_an_error() {
    let mut app = App::new();
    app.start().await.unwrap();
    assert!(matches!(app.start().await, Err(Error::AlreadyStarted)));
}

#[tokio::test(start_paused = true)]
async fn start_and_stop_are_bounded_by_their_timeouts() {
    let mut slow_start = App::new()
        .start_timeout(Duration::from_secs(1))
        .constructor(|life: Lifecycle| {
            life.on_start(|| tokio::time::sleep(Duration::from_secs(60)));
            Arc::new(Config(""))
        })
        .invoke(|_: Arc<Config>| {});
    let error = slow_start.start().await.unwrap_err();
    assert!(
        matches!(
            error,
            Error::Timeout {
                phase: Phase::Start,
                ..
            }
        ),
        "{error}"
    );

    let mut slow_stop = App::new()
        .stop_timeout(Duration::from_secs(1))
        .constructor(|life: Lifecycle| {
            life.on_stop(|| tokio::time::sleep(Duration::from_secs(60)));
            Arc::new(Config(""))
        })
        .invoke(|_: Arc<Config>| {});
    slow_stop.start().await.unwrap();
    let error = slow_stop.stop().await.unwrap_err();
    assert!(matches!(error, Error::Timeout { phase: Phase::Stop, .. }), "{error}");
}

// ---- shutdown -------------------------------------------------------------------

#[tokio::test]
async fn run_waits_for_a_shutdown_request_and_returns_its_exit_code() {
    let log = log();
    let l = log.clone();
    let code = App::new()
        .constructor(move |life: Lifecycle| {
            life.on_stop({
                let l = l.clone();
                move || async move { push(&l, "stopped") }
            });
            Arc::new(Config(""))
        })
        .invoke(|_: Arc<Config>, shutdown: Shutdowner| {
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(20)).await;
                shutdown.shutdown_with(3);
                shutdown.shutdown_with(9); // the first request wins
            });
        })
        .run()
        .await
        .unwrap();
    assert_eq!(code, 3);
    assert_eq!(lines(&log), ["stopped"]);
}

#[tokio::test]
async fn done_resolves_for_a_request_made_through_the_apps_own_handle() {
    let app = App::new();
    let handle = app.shutdowner();
    handle.shutdown();
    assert_eq!(app.done().await, fx::Signal::Requested(0));
}

// ---- replace --------------------------------------------------------------------

#[tokio::test]
async fn replace_swaps_a_provider_and_drops_its_dependencies() {
    let log = log();
    let l = log.clone();
    let mut app = App::new()
        // `database` would need a Config nobody provides; replacing it removes that need.
        .constructor(database)
        .constructor(move |db: Arc<Database>| {
            push(&l, format!("repo over {}", db.0.0));
            Arc::new(Repo(db))
        })
        .replace(Arc::new(Database(Arc::new(Config("fake")))))
        .invoke(|_: Arc<Repo>| {});
    app.start().await.unwrap();
    assert_eq!(lines(&log), ["repo over fake"]);
}

#[test]
fn replacing_something_nobody_provides_is_an_error() {
    let error = App::new().replace(Arc::new(Config("x"))).validate().unwrap_err();
    assert!(matches!(error, Error::UnknownReplace { .. }), "{error}");
}

// ---- reuse ----------------------------------------------------------------------

#[tokio::test]
async fn options_bundle_and_merge() {
    let data = Options::new().constructor(config).constructor(database);
    let more = Options::new().constructor(repo).invoke(|_: Arc<Repo>| {});
    let app = App::new().options(data).options(more);
    app.validate().unwrap();
    app.run_until_started().await.unwrap();
}

#[tokio::test]
async fn get_before_start_is_an_error() {
    let mut app = App::new().constructor(config);
    assert!(matches!(app.get::<Config>().await, Err(Error::NotStarted)));
}

#[tokio::test]
async fn errors_carry_the_source_location_of_the_provider() {
    let error = App::new().constructor(repo).validate().unwrap_err();
    // Missing names the requester, whose origin includes file:line of this test file.
    assert!(error.to_string().contains("core.rs:"), "{error}");
}
