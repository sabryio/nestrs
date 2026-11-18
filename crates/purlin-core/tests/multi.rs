//! Multi-binding: many implementations of one interface, collected as `Multi<I>`.

use std::future::Future;
use std::sync::Arc;

use purlin_core::*;

trait Check: Send + Sync {
    fn name(&self) -> String;
}

struct Disk;
struct Cache {
    prefix: Arc<Prefix>,
}
struct Prefix(&'static str);

impl Check for Disk {
    fn name(&self) -> String {
        "disk".into()
    }
}
impl Check for Cache {
    fn name(&self) -> String {
        format!("{}cache", self.prefix.0)
    }
}

impl Injectable for Disk {
    fn dependencies() -> Vec<Token> {
        vec![]
    }
    async fn build(_: &Container) -> Result<Self, DiError> {
        Ok(Disk)
    }
}
impl Injectable for Cache {
    fn dependencies() -> Vec<Token> {
        vec![Token::of::<Prefix>()]
    }
    fn build(c: &Container) -> impl Future<Output = Result<Self, DiError>> + Send {
        let prefix = c.resolve::<Prefix>();
        async move { Ok(Cache { prefix: prefix? }) }
    }
}

/// Consumes the whole set.
struct Report {
    checks: Arc<Multi<dyn Check>>,
}
impl Injectable for Report {
    fn dependencies() -> Vec<Token> {
        vec![Token::of::<Multi<dyn Check>>()]
    }
    fn build(c: &Container) -> impl Future<Output = Result<Self, DiError>> + Send {
        let checks = c.resolve::<Multi<dyn Check>>();
        async move { Ok(Report { checks: checks? }) }
    }
}

fn names(multi: &Multi<dyn Check>) -> Vec<String> {
    multi.iter().map(|check| check.name()).collect()
}

/// Contributes from two modules; the consumer imports neither.
struct DiskModule;
impl Module for DiskModule {
    fn definition() -> ModuleDef {
        ModuleDef::new("DiskModule").providers([contribute!(dyn Check => Disk)])
    }
}
struct CacheModule;
impl Module for CacheModule {
    fn definition() -> ModuleDef {
        ModuleDef::new("CacheModule").providers([Provider::value(Prefix("fast-")), contribute!(dyn Check => Cache)])
    }
}
struct Root;
impl Module for Root {
    fn definition() -> ModuleDef {
        ModuleDef::new("Root")
            .imports([ModuleRef::of::<DiskModule>(), ModuleRef::of::<CacheModule>()])
            .providers([Provider::of::<Report>()])
    }
}

#[tokio::test]
async fn members_from_different_modules_are_collected_in_registration_order() {
    let booted = bootstrap::<Root>().await.unwrap();
    let report = booted.container.resolve::<Report>().unwrap();
    assert_eq!(names(&report.checks), ["disk", "fast-cache"]);
    // The members' own dependencies stayed private to their module.
    assert!(booted.container.resolve::<Multi<dyn Check>>().is_ok());
}

struct Lonely;
impl Module for Lonely {
    fn definition() -> ModuleDef {
        ModuleDef::new("Lonely").providers([
            Provider::multi::<dyn Check>(),
            Provider::multi::<dyn Check>(),
            Provider::of::<Report>(),
        ])
    }
}

#[tokio::test]
async fn a_declared_set_may_be_empty_and_may_be_declared_twice() {
    let booted = bootstrap::<Lonely>().await.unwrap();
    assert!(booted.container.resolve::<Report>().unwrap().checks.is_empty());
}

struct Missing;
impl Module for Missing {
    fn definition() -> ModuleDef {
        ModuleDef::new("Missing").providers([Provider::of::<Report>()])
    }
}

#[tokio::test]
async fn an_undeclared_set_nobody_contributes_to_is_a_missing_provider() {
    let error = bootstrap::<Missing>().await.unwrap_err();
    assert!(matches!(error, DiError::MissingProvider { .. }), "{error}");
}

struct Fake;
impl Check for Fake {
    fn name(&self) -> String {
        "fake".into()
    }
}

#[tokio::test]
async fn tests_replace_the_whole_set() {
    let fake: Arc<dyn Check> = Arc::new(Fake);
    let overrides = Overrides::new().provide(Provider::value(Multi::<dyn Check>::new(vec![fake])));
    let booted = bootstrap_with::<Root>(overrides).await.unwrap();
    let report = booted.container.resolve::<Report>().unwrap();
    assert_eq!(names(&report.checks), ["fake"]);
}

#[tokio::test]
async fn a_member_whose_dependency_is_missing_is_reported_like_any_provider() {
    struct Broken;
    impl Module for Broken {
        fn definition() -> ModuleDef {
            ModuleDef::new("Broken").providers([contribute!(dyn Check => Cache), Provider::of::<Report>()])
        }
    }
    let error = bootstrap::<Broken>().await.unwrap_err();
    assert!(matches!(error, DiError::MissingProvider { .. }), "{error}");
}
