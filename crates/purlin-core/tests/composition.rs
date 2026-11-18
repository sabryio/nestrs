//! Overrides, configured (dynamic) modules and global modules, tested through the
//! public API with hand-written modules (no macros).

// Spelling out `-> impl Future<..> + Send` mirrors the `Injectable::build` contract.
#![allow(clippy::manual_async_fn)]

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use purlin_core::*;

// ---------------------------------------------------------------- fixtures

struct Config {
    name: String,
}

impl Config {
    fn named(name: &str) -> Self {
        Self { name: name.to_owned() }
    }
}

trait Repo: Send + Sync {
    fn kind(&self) -> &'static str;
}

struct MemRepo;
impl Repo for MemRepo {
    fn kind(&self) -> &'static str {
        "mem"
    }
}
impl Injectable for MemRepo {
    fn dependencies() -> Vec<Token> {
        vec![]
    }
    fn build(_: &Container) -> impl Future<Output = Result<Self, DiError>> + Send {
        async { Ok(Self) }
    }
}

struct FakeRepo;
impl Repo for FakeRepo {
    fn kind(&self) -> &'static str {
        "fake"
    }
}

struct Service {
    repo: Arc<dyn Repo>,
    config: Arc<Config>,
}
impl Injectable for Service {
    fn dependencies() -> Vec<Token> {
        vec![Token::of::<dyn Repo>(), Token::of::<Config>()]
    }
    fn build(c: &Container) -> impl Future<Output = Result<Self, DiError>> + Send {
        async move {
            Ok(Self {
                repo: c.resolve::<dyn Repo>()?,
                config: c.resolve::<Config>()?,
            })
        }
    }
}

/// A module with everything wired inside it.
struct App;
impl Module for App {
    fn definition() -> ModuleDef {
        ModuleDef::new("App").providers([
            Provider::value(Config::named("real")),
            bind!(dyn Repo => MemRepo),
            Provider::of::<Service>(),
        ])
    }
}

/// Anything that needs `Config` and nothing else.
struct NeedsConfig {
    config: Arc<Config>,
}
impl Injectable for NeedsConfig {
    fn dependencies() -> Vec<Token> {
        vec![Token::of::<Config>()]
    }
    fn build(c: &Container) -> impl Future<Output = Result<Self, DiError>> + Send {
        async move {
            Ok(Self {
                config: c.resolve::<Config>()?,
            })
        }
    }
}

fn config_module(name: &str) -> ModuleRef {
    ModuleDef::new("ConfigModule")
        .providers([Provider::value(Config::named(name))])
        .exports([Token::of::<Config>()])
        .into_ref()
}

fn global_config_module(name: &str) -> ModuleRef {
    ModuleDef::new("ConfigModule")
        .global()
        .providers([Provider::value(Config::named(name))])
        .exports([Token::of::<Config>()])
        .into_ref()
}

// ---------------------------------------------------------------- overrides

#[tokio::test]
async fn an_override_replaces_the_provider_for_every_dependent() {
    let overrides = Overrides::new().provide(Provider::value(Config::named("fake")));
    let booted = bootstrap_with::<App>(overrides).await.unwrap();

    assert_eq!(booted.container.resolve::<Config>().unwrap().name, "fake");
    assert_eq!(booted.container.resolve::<Service>().unwrap().config.name, "fake");
}

#[tokio::test]
async fn trait_objects_are_overridden_with_an_instance_the_test_still_holds() {
    let fake = Arc::new(FakeRepo);
    let overrides = Overrides::new().provide(Provider::instance::<dyn Repo>(fake.clone()));
    let booted = bootstrap_with::<App>(overrides).await.unwrap();

    let service = booted.container.resolve::<Service>().unwrap();
    assert_eq!(service.repo.kind(), "fake");
    assert!(
        Arc::strong_count(&fake) >= 2,
        "the container shares the test's instance"
    );
}

#[tokio::test]
async fn an_overridden_provider_is_replaced_not_wrapped() {
    static ORIGINAL_BUILDS: AtomicUsize = AtomicUsize::new(0);

    struct Counting;
    impl Module for Counting {
        fn definition() -> ModuleDef {
            ModuleDef::new("Counting").providers([Provider::factory::<Config>(vec![], |_| {
                Box::pin(async {
                    ORIGINAL_BUILDS.fetch_add(1, Ordering::SeqCst);
                    Ok(Config::named("original"))
                })
            })])
        }
    }

    let overrides = Overrides::new().provide(Provider::value(Config::named("replacement")));
    let booted = bootstrap_with::<Counting>(overrides).await.unwrap();

    assert_eq!(booted.container.resolve::<Config>().unwrap().name, "replacement");
    assert_eq!(
        ORIGINAL_BUILDS.load(Ordering::SeqCst),
        0,
        "the original factory must not run"
    );
}

#[tokio::test]
async fn the_last_override_for_a_token_wins() {
    let overrides = Overrides::new()
        .provide(Provider::value(Config::named("first")))
        .provide(Provider::value(Config::named("second")));
    let booted = bootstrap_with::<App>(overrides).await.unwrap();
    assert_eq!(booted.container.resolve::<Config>().unwrap().name, "second");
}

#[tokio::test]
async fn overriding_something_nobody_provides_is_an_error() {
    struct Unrelated;
    let overrides = Overrides::new().provide(Provider::value(Unrelated));

    let error = bootstrap_with::<App>(overrides).await.unwrap_err();
    assert!(matches!(error, DiError::UnknownOverride { .. }), "{error}");
    assert!(error.to_string().contains("Unrelated"), "{error}");
}

#[tokio::test]
async fn an_override_keeps_the_original_modules_exports() {
    // ConfigModule exports Config; the override must not change who can see it.
    struct Root;
    impl Module for Root {
        fn definition() -> ModuleDef {
            ModuleDef::new("Root")
                .imports([config_module("real")])
                .providers([Provider::of::<NeedsConfig>()])
        }
    }

    let overrides = Overrides::new().provide(Provider::value(Config::named("fake")));
    let booted = bootstrap_with::<Root>(overrides).await.unwrap();
    assert_eq!(booted.container.resolve::<NeedsConfig>().unwrap().config.name, "fake");
}

#[tokio::test]
async fn override_dependencies_must_exist() {
    struct Missing;
    let overrides = Overrides::new().provide(Provider::factory::<Config>(vec![Token::of::<Missing>()], |_| {
        Box::pin(async { Ok(Config::named("never built")) })
    }));

    let error = bootstrap_with::<App>(overrides).await.unwrap_err();
    assert!(matches!(error, DiError::MissingProvider { .. }), "{error}");
}

#[tokio::test]
async fn override_dependencies_are_exempt_from_module_visibility() {
    struct Secret(&'static str);

    // `Secret` is provided but deliberately not exported.
    struct Private;
    impl Module for Private {
        fn definition() -> ModuleDef {
            ModuleDef::new("Private").providers([Provider::value(Secret("from the private module"))])
        }
    }
    struct Root;
    impl Module for Root {
        fn definition() -> ModuleDef {
            ModuleDef::new("Root")
                .imports([ModuleRef::of::<Private>()])
                .providers([Provider::value(Config::named("real"))])
        }
    }

    let reach_in = Provider::factory::<Config>(vec![Token::of::<Secret>()], |container| {
        Box::pin(async move {
            let secret = container.resolve::<Secret>()?;
            Ok(Config::named(secret.0))
        })
    });
    let booted = bootstrap_with::<Root>(Overrides::new().provide(reach_in))
        .await
        .unwrap();
    assert_eq!(
        booted.container.resolve::<Config>().unwrap().name,
        "from the private module"
    );
}

// --------------------------------------------------------- configured modules

#[tokio::test]
async fn configured_modules_carry_their_configuration() {
    struct Root;
    impl Module for Root {
        fn definition() -> ModuleDef {
            ModuleDef::new("Root")
                .imports([config_module("production")])
                .providers([Provider::of::<NeedsConfig>()])
        }
    }

    let booted = bootstrap::<Root>().await.unwrap();
    assert_eq!(
        booted.container.resolve::<NeedsConfig>().unwrap().config.name,
        "production"
    );
}

#[tokio::test]
async fn separately_configured_instances_conflict_instead_of_one_silently_winning() {
    struct Root;
    impl Module for Root {
        fn definition() -> ModuleDef {
            ModuleDef::new("Root").imports([config_module("a"), config_module("b")])
        }
    }

    let error = bootstrap::<Root>().await.unwrap_err();
    assert!(
        matches!(
            error,
            DiError::DuplicateProvider {
                first: "ConfigModule",
                second: "ConfigModule",
                ..
            }
        ),
        "{error}"
    );
}

#[tokio::test]
async fn cloning_a_module_ref_shares_one_instance() {
    static BUILDS: AtomicUsize = AtomicUsize::new(0);

    struct Root;
    impl Module for Root {
        fn definition() -> ModuleDef {
            let shared = ModuleDef::new("Shared")
                .providers([Provider::factory::<Config>(vec![], |_| {
                    Box::pin(async {
                        BUILDS.fetch_add(1, Ordering::SeqCst);
                        Ok(Config::named("shared"))
                    })
                })])
                .exports([Token::of::<Config>()])
                .into_ref();
            let left = ModuleDef::new("Left").imports([shared.clone()]).into_ref();
            let right = ModuleDef::new("Right").imports([shared]).into_ref();
            ModuleDef::new("Root").imports([left, right])
        }
    }

    bootstrap::<Root>().await.unwrap();
    assert_eq!(BUILDS.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn each_bootstrap_builds_fresh_instances_from_the_same_definition() {
    struct Root;
    impl Module for Root {
        fn definition() -> ModuleDef {
            ModuleDef::new("Root").imports([config_module("x")])
        }
    }

    let first = bootstrap::<Root>().await.unwrap();
    let second = bootstrap::<Root>().await.unwrap();
    let (a, b) = (
        first.container.resolve::<Config>().unwrap(),
        second.container.resolve::<Config>().unwrap(),
    );
    assert!(!Arc::ptr_eq(&a, &b));
}

// ----------------------------------------------------------- global modules

/// Feature module that never imports the config module.
struct Feature;
impl Module for Feature {
    fn definition() -> ModuleDef {
        ModuleDef::new("Feature").providers([Provider::of::<NeedsConfig>()])
    }
}

#[tokio::test]
async fn global_modules_are_visible_without_an_import() {
    struct Root;
    impl Module for Root {
        fn definition() -> ModuleDef {
            ModuleDef::new("Root").imports([global_config_module("g"), ModuleRef::of::<Feature>()])
        }
    }

    let booted = bootstrap::<Root>().await.unwrap();
    assert_eq!(booted.container.resolve::<NeedsConfig>().unwrap().config.name, "g");
}

#[tokio::test]
async fn without_global_the_same_wiring_is_rejected() {
    struct Root;
    impl Module for Root {
        fn definition() -> ModuleDef {
            ModuleDef::new("Root").imports([config_module("g"), ModuleRef::of::<Feature>()])
        }
    }

    let error = bootstrap::<Root>().await.unwrap_err();
    assert!(
        matches!(
            error,
            DiError::NotVisible {
                module: "Feature",
                provided_in: "ConfigModule",
                ..
            }
        ),
        "{error}"
    );
    assert!(
        error.to_string().contains("global"),
        "the message should mention the fix: {error}"
    );
}

#[tokio::test]
async fn a_global_module_still_has_to_be_part_of_the_graph() {
    struct Root;
    impl Module for Root {
        fn definition() -> ModuleDef {
            // The global module exists in the program but nobody imports it.
            let _unused = global_config_module("g");
            ModuleDef::new("Root").imports([ModuleRef::of::<Feature>()])
        }
    }

    let error = bootstrap::<Root>().await.unwrap_err();
    assert!(
        matches!(error, DiError::MissingProvider { module: "Feature", .. }),
        "{error}"
    );
}

#[tokio::test]
async fn global_modules_expose_only_what_they_export() {
    struct Secret;
    struct NeedsSecret {
        _secret: Arc<Secret>,
    }
    impl Injectable for NeedsSecret {
        fn dependencies() -> Vec<Token> {
            vec![Token::of::<Secret>()]
        }
        fn build(c: &Container) -> impl Future<Output = Result<Self, DiError>> + Send {
            async move {
                Ok(Self {
                    _secret: c.resolve::<Secret>()?,
                })
            }
        }
    }
    struct Greedy;
    impl Module for Greedy {
        fn definition() -> ModuleDef {
            ModuleDef::new("Greedy").providers([Provider::of::<NeedsSecret>()])
        }
    }
    struct Root;
    impl Module for Root {
        fn definition() -> ModuleDef {
            let global = ModuleDef::new("GlobalModule")
                .global()
                .providers([Provider::value(Config::named("g")), Provider::value(Secret)])
                .exports([Token::of::<Config>()])
                .into_ref();
            ModuleDef::new("Root").imports([global, ModuleRef::of::<Greedy>()])
        }
    }

    let error = bootstrap::<Root>().await.unwrap_err();
    assert!(
        matches!(
            error,
            DiError::NotVisible {
                module: "Greedy",
                provided_in: "GlobalModule",
                ..
            }
        ),
        "{error}"
    );
}
