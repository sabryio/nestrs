//! Behavioural tests for graph validation and bootstrap, written against the public
//! API only (modules and injectables are implemented by hand, no macros).

// The fixtures spell out `-> impl Future<..> + Send` on purpose: that is exactly the
// contract `Injectable::build` declares, and `#[injectable]` generates the same shape.
#![allow(clippy::manual_async_fn)]

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use nestrs_core::*;

// ---------------------------------------------------------------- fixtures

struct Config {
    name: &'static str,
}

trait Repo: Send + Sync {
    fn kind(&self) -> &'static str;
}

struct MemRepo {
    #[allow(dead_code)]
    config: Arc<Config>,
}

impl Repo for MemRepo {
    fn kind(&self) -> &'static str {
        "mem"
    }
}

impl Injectable for MemRepo {
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

struct Service {
    repo: Arc<dyn Repo>,
}

impl Injectable for Service {
    fn dependencies() -> Vec<Token> {
        vec![Token::of::<dyn Repo>()]
    }
    fn build(c: &Container) -> impl Future<Output = Result<Self, DiError>> + Send {
        async move {
            Ok(Self {
                repo: c.resolve::<dyn Repo>()?,
            })
        }
    }
}

struct ConfigModule;
impl Module for ConfigModule {
    fn definition() -> ModuleDef {
        ModuleDef::new("ConfigModule")
            .providers([Provider::value(Config { name: "test" })])
            .exports([Token::of::<Config>()])
    }
}

/// Provides the repo but (deliberately) exports it only when asked to.
struct DataModule;
impl Module for DataModule {
    fn definition() -> ModuleDef {
        ModuleDef::new("DataModule")
            .imports([ModuleRef::of::<ConfigModule>()])
            .providers([bind!(dyn Repo => MemRepo)])
            .exports([Token::of::<dyn Repo>()])
    }
}

struct PrivateDataModule;
impl Module for PrivateDataModule {
    fn definition() -> ModuleDef {
        ModuleDef::new("PrivateDataModule")
            .imports([ModuleRef::of::<ConfigModule>()])
            .providers([bind!(dyn Repo => MemRepo)])
    }
}

// ------------------------------------------------------------ happy paths

#[tokio::test]
async fn resolves_trait_objects_across_modules() {
    struct App;
    impl Module for App {
        fn definition() -> ModuleDef {
            ModuleDef::new("App")
                .imports([ModuleRef::of::<DataModule>()])
                .providers([Provider::of::<Service>()])
        }
    }

    let booted = bootstrap::<App>().await.unwrap();
    let service = booted.container.resolve::<Service>().unwrap();
    assert_eq!(service.repo.kind(), "mem");
    assert_eq!(booted.modules, ["ConfigModule", "DataModule", "App"]);
}

#[tokio::test]
async fn declaration_order_does_not_matter() {
    struct App;
    impl Module for App {
        fn definition() -> ModuleDef {
            ModuleDef::new("App").providers([
                Provider::of::<Service>(),
                bind!(dyn Repo => MemRepo),
                Provider::value(Config { name: "late" }),
            ])
        }
    }

    let booted = bootstrap::<App>().await.unwrap();
    let repo = booted.container.resolve::<dyn Repo>().unwrap();
    assert_eq!(repo.kind(), "mem");
    assert_eq!(booted.container.resolve::<Config>().unwrap().name, "late");
}

#[tokio::test]
async fn re_exports_pass_through_intermediate_modules() {
    struct Gateway;
    impl Module for Gateway {
        fn definition() -> ModuleDef {
            ModuleDef::new("Gateway")
                .imports([ModuleRef::of::<ConfigModule>()])
                .exports([Token::of::<Config>()])
        }
    }
    struct App;
    impl Module for App {
        fn definition() -> ModuleDef {
            ModuleDef::new("App")
                .imports([ModuleRef::of::<Gateway>()])
                .providers([bind!(dyn Repo => MemRepo)])
        }
    }

    let booted = bootstrap::<App>().await.unwrap();
    assert_eq!(booted.container.resolve::<Config>().unwrap().name, "test");
}

#[tokio::test]
async fn a_module_imported_twice_is_built_once() {
    static BUILDS: AtomicUsize = AtomicUsize::new(0);
    struct Counted;

    struct Shared;
    impl Module for Shared {
        fn definition() -> ModuleDef {
            ModuleDef::new("Shared")
                .providers([Provider::factory::<Counted>(vec![], |_| {
                    Box::pin(async {
                        BUILDS.fetch_add(1, Ordering::SeqCst);
                        Ok(Counted)
                    })
                })])
                .exports([Token::of::<Counted>()])
        }
    }
    struct Left;
    impl Module for Left {
        fn definition() -> ModuleDef {
            ModuleDef::new("Left").imports([ModuleRef::of::<Shared>()])
        }
    }
    struct Right;
    impl Module for Right {
        fn definition() -> ModuleDef {
            ModuleDef::new("Right").imports([ModuleRef::of::<Shared>()])
        }
    }
    struct App;
    impl Module for App {
        fn definition() -> ModuleDef {
            ModuleDef::new("App").imports([ModuleRef::of::<Left>(), ModuleRef::of::<Right>()])
        }
    }

    bootstrap::<App>().await.unwrap();
    assert_eq!(BUILDS.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn async_factories_can_await() {
    struct App;
    impl Module for App {
        fn definition() -> ModuleDef {
            ModuleDef::new("App").providers([Provider::factory::<String>(vec![], |_| {
                Box::pin(async {
                    tokio::task::yield_now().await;
                    Ok("ready".to_owned())
                })
            })])
        }
    }

    let booted = bootstrap::<App>().await.unwrap();
    assert_eq!(*booted.container.resolve::<String>().unwrap(), "ready");
}

// ------------------------------------------------------------ wiring errors

#[tokio::test]
async fn missing_provider_is_reported_with_both_names() {
    struct App;
    impl Module for App {
        fn definition() -> ModuleDef {
            ModuleDef::new("App").providers([Provider::of::<Service>()])
        }
    }

    let error = bootstrap::<App>().await.unwrap_err();
    assert!(
        matches!(error, DiError::MissingProvider { module: "App", .. }),
        "{error}"
    );
    let message = error.to_string();
    assert!(message.contains("Service") && message.contains("dyn Repo"), "{message}");
}

#[tokio::test]
async fn unexported_providers_are_not_visible_to_importers() {
    struct App;
    impl Module for App {
        fn definition() -> ModuleDef {
            ModuleDef::new("App")
                .imports([ModuleRef::of::<PrivateDataModule>()])
                .providers([Provider::of::<Service>()])
        }
    }

    let error = bootstrap::<App>().await.unwrap_err();
    assert!(
        matches!(
            error,
            DiError::NotVisible {
                module: "App",
                provided_in: "PrivateDataModule",
                ..
            }
        ),
        "{error}"
    );
}

#[tokio::test]
async fn importing_without_the_export_is_not_enough_transitively() {
    // Config is exported by ConfigModule, but DataModule does not re-export it, so
    // App (which only imports DataModule) cannot depend on Config directly.
    struct NeedsConfig;
    impl Injectable for NeedsConfig {
        fn dependencies() -> Vec<Token> {
            vec![Token::of::<Config>()]
        }
        fn build(c: &Container) -> impl Future<Output = Result<Self, DiError>> + Send {
            async move {
                c.resolve::<Config>()?;
                Ok(Self)
            }
        }
    }
    struct App;
    impl Module for App {
        fn definition() -> ModuleDef {
            ModuleDef::new("App")
                .imports([ModuleRef::of::<DataModule>()])
                .providers([Provider::of::<NeedsConfig>()])
        }
    }

    let error = bootstrap::<App>().await.unwrap_err();
    assert!(
        matches!(
            error,
            DiError::NotVisible {
                provided_in: "ConfigModule",
                ..
            }
        ),
        "{error}"
    );
}

#[tokio::test]
async fn duplicate_providers_across_modules_are_rejected() {
    struct Other;
    impl Module for Other {
        fn definition() -> ModuleDef {
            ModuleDef::new("Other").providers([Provider::value(Config { name: "dup" })])
        }
    }
    struct App;
    impl Module for App {
        fn definition() -> ModuleDef {
            ModuleDef::new("App").imports([ModuleRef::of::<ConfigModule>(), ModuleRef::of::<Other>()])
        }
    }

    let error = bootstrap::<App>().await.unwrap_err();
    assert!(
        matches!(
            error,
            DiError::DuplicateProvider {
                first: "ConfigModule",
                second: "Other",
                ..
            }
        ),
        "{error}"
    );
}

#[tokio::test]
async fn exporting_something_you_do_not_have_is_rejected() {
    struct App;
    impl Module for App {
        fn definition() -> ModuleDef {
            ModuleDef::new("App").exports([Token::of::<Config>()])
        }
    }

    let error = bootstrap::<App>().await.unwrap_err();
    assert!(matches!(error, DiError::InvalidExport { module: "App", .. }), "{error}");
}

#[tokio::test]
async fn circular_module_imports_are_rejected() {
    struct A;
    struct B;
    impl Module for A {
        fn definition() -> ModuleDef {
            ModuleDef::new("A").imports([ModuleRef::of::<B>()])
        }
    }
    impl Module for B {
        fn definition() -> ModuleDef {
            ModuleDef::new("B").imports([ModuleRef::of::<A>()])
        }
    }

    let error = bootstrap::<A>().await.unwrap_err();
    match error {
        DiError::ModuleCycle { path } => assert_eq!(path, ["A", "B", "A"]),
        other => panic!("unexpected error: {other}"),
    }
}

#[tokio::test]
async fn circular_provider_dependencies_are_rejected() {
    struct X;
    struct Y;
    impl Injectable for X {
        fn dependencies() -> Vec<Token> {
            vec![Token::of::<Y>()]
        }
        fn build(_: &Container) -> impl Future<Output = Result<Self, DiError>> + Send {
            async { Ok(Self) }
        }
    }
    impl Injectable for Y {
        fn dependencies() -> Vec<Token> {
            vec![Token::of::<X>()]
        }
        fn build(_: &Container) -> impl Future<Output = Result<Self, DiError>> + Send {
            async { Ok(Self) }
        }
    }
    struct App;
    impl Module for App {
        fn definition() -> ModuleDef {
            ModuleDef::new("App").providers([Provider::of::<X>(), Provider::of::<Y>()])
        }
    }

    let error = bootstrap::<App>().await.unwrap_err();
    match error {
        DiError::CircularDependency { path } => assert_eq!(path, ["X", "Y", "X"]),
        other => panic!("unexpected error: {other}"),
    }
}

// -------------------------------------------------------- runtime failures

#[tokio::test]
async fn factory_errors_are_attributed_to_their_provider() {
    struct Flaky;
    struct App;
    impl Module for App {
        fn definition() -> ModuleDef {
            ModuleDef::new("App").providers([Provider::factory::<Flaky>(vec![], |_| {
                Box::pin(async { Err(DiError::custom("connection refused")) })
            })])
        }
    }

    let error = bootstrap::<App>().await.unwrap_err();
    match error {
        DiError::Factory { provider, message } => {
            assert_eq!(provider, Token::of::<Flaky>());
            assert_eq!(message, "connection refused");
        }
        other => panic!("unexpected error: {other}"),
    }
}

// ------------------------------------------------------------ transports

struct TestTransport;
impl Transport for TestTransport {
    type Mounted = String;
}
struct OtherTransport;
impl Transport for OtherTransport {
    type Mounted = u8;
}

struct Ctl {
    repo: Arc<dyn Repo>,
}
impl Injectable for Ctl {
    fn dependencies() -> Vec<Token> {
        vec![Token::of::<dyn Repo>()]
    }
    fn build(c: &Container) -> impl Future<Output = Result<Self, DiError>> + Send {
        async move {
            Ok(Self {
                repo: c.resolve::<dyn Repo>()?,
            })
        }
    }
}
impl Controller for Ctl {
    fn entry() -> ControllerEntry {
        ControllerEntry::new::<TestTransport>(Provider::of::<Ctl>(), |c| {
            let controller = c.resolve::<Ctl>()?;
            Ok(format!("mounted with {}", controller.repo.kind()))
        })
    }
}

#[tokio::test]
async fn controllers_are_mounted_per_transport() {
    struct App;
    impl Module for App {
        fn definition() -> ModuleDef {
            ModuleDef::new("App")
                .imports([ModuleRef::of::<DataModule>()])
                .controllers([Ctl::entry()])
        }
    }

    let mut booted = bootstrap::<App>().await.unwrap();
    assert_eq!(booted.controllers.len(), 1);
    let mounted = booted.controllers.pop().unwrap();

    let mounted = mounted.for_transport::<OtherTransport>().unwrap_err();
    assert_eq!(mounted.controller, Token::of::<Ctl>());
    assert_eq!(mounted.for_transport::<TestTransport>().unwrap(), "mounted with mem");
}

#[tokio::test]
async fn controllers_obey_the_same_visibility_rules() {
    struct App;
    impl Module for App {
        fn definition() -> ModuleDef {
            ModuleDef::new("App")
                .imports([ModuleRef::of::<PrivateDataModule>()])
                .controllers([Ctl::entry()])
        }
    }

    let error = bootstrap::<App>().await.unwrap_err();
    assert!(matches!(error, DiError::NotVisible { module: "App", .. }), "{error}");
}
