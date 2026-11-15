//! Request-scoped and transient providers, and the lifetime rule.

#![allow(clippy::manual_async_fn)]

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use nestrs_core::*;

static BUILT: AtomicUsize = AtomicUsize::new(0);

/// Singleton.
#[derive(Debug)]
struct Config;
impl Injectable for Config {
    fn dependencies() -> Vec<Token> {
        vec![]
    }
    fn build(_: &Container) -> impl Future<Output = Result<Self, DiError>> + Send {
        async { Ok(Self) }
    }
}

/// Request-scoped, depends on a singleton and a seeded value.
#[derive(Debug)]
struct RequestContext {
    id: usize,
    user: Arc<Caller>,
    _config: Arc<Config>,
}
#[derive(Debug)]
struct Caller(&'static str);
impl Injectable for RequestContext {
    fn dependencies() -> Vec<Token> {
        vec![Token::of::<Config>(), Token::of::<Caller>()]
    }
    fn build(c: &Container) -> impl Future<Output = Result<Self, DiError>> + Send {
        let built = (c.resolve::<Config>(), c.resolve::<Caller>());
        async move {
            let (config, user) = built;
            Ok(Self {
                id: BUILT.fetch_add(1, Ordering::SeqCst),
                user: user?,
                _config: config?,
            })
        }
    }
}

/// Transient, depends on a request-scoped provider.
struct Handle {
    context: Arc<RequestContext>,
    serial: usize,
}
impl Injectable for Handle {
    fn dependencies() -> Vec<Token> {
        vec![Token::of::<RequestContext>()]
    }
    fn build(c: &Container) -> impl Future<Output = Result<Self, DiError>> + Send {
        let context = c.resolve::<RequestContext>();
        async move {
            Ok(Self {
                context: context?,
                serial: BUILT.fetch_add(1, Ordering::SeqCst),
            })
        }
    }
}

/// A singleton that (wrongly) depends on a request-scoped provider.
struct Greedy;
impl Injectable for Greedy {
    fn dependencies() -> Vec<Token> {
        vec![Token::of::<RequestContext>()]
    }
    fn build(_: &Container) -> impl Future<Output = Result<Self, DiError>> + Send {
        async { Ok(Self) }
    }
}

fn scoped_providers() -> [Provider; 4] {
    [
        Provider::of::<Config>(),
        Provider::seeded::<Caller>(),
        Provider::of::<RequestContext>().request(),
        Provider::of::<Handle>().transient(),
    ]
}

struct AppModule;
impl Module for AppModule {
    fn definition() -> ModuleDef {
        ModuleDef::new("AppModule").providers(scoped_providers())
    }
}

async fn scope(booted: &Bootstrapped, caller: &'static str) -> RequestScope {
    let scope = RequestScope::new(booted.container.clone(), booted.scoped.clone());
    scope.seed(Arc::new(Caller(caller)));
    scope
}

#[tokio::test]
async fn request_scoped_providers_are_built_once_per_scope() {
    let booted = bootstrap::<AppModule>().await.unwrap();
    assert!(!booted.container.contains::<RequestContext>(), "not built at startup");

    let scope = scope(&booted, "ada").await;
    let first = scope.resolve::<RequestContext>().await.unwrap();
    let second = scope.resolve::<RequestContext>().await.unwrap();
    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(first.user.0, "ada");

    let other = self::scope(&booted, "grace").await;
    let third = other.resolve::<RequestContext>().await.unwrap();
    assert!(!Arc::ptr_eq(&first, &third), "a new request gets its own instance");
    assert_eq!(third.user.0, "grace");
}

#[tokio::test]
async fn transient_providers_are_built_every_time_but_share_the_scopes_request_state() {
    let booted = bootstrap::<AppModule>().await.unwrap();
    let scope = scope(&booted, "ada").await;
    let a = scope.resolve::<Handle>().await.unwrap();
    let b = scope.resolve::<Handle>().await.unwrap();
    assert!(!Arc::ptr_eq(&a, &b));
    assert_ne!(a.serial, b.serial);
    assert!(Arc::ptr_eq(&a.context, &b.context));
    assert_eq!(a.context.id, b.context.id);
}

#[tokio::test]
async fn singletons_resolve_through_the_scope_too() {
    let booted = bootstrap::<AppModule>().await.unwrap();
    let scope = scope(&booted, "ada").await;
    let from_scope = scope.resolve::<Config>().await.unwrap();
    assert!(Arc::ptr_eq(&from_scope, &booted.container.resolve::<Config>().unwrap()));
}

#[tokio::test]
async fn an_unseeded_value_is_a_clear_error() {
    let booted = bootstrap::<AppModule>().await.unwrap();
    let scope = RequestScope::new(booted.container.clone(), booted.scoped.clone());
    let error = scope.resolve::<RequestContext>().await.unwrap_err();
    assert!(error.to_string().contains("seeded per request"), "{error}");
}

struct GreedyModule;
impl Module for GreedyModule {
    fn definition() -> ModuleDef {
        ModuleDef::new("GreedyModule")
            .providers(scoped_providers())
            .providers([Provider::of::<Greedy>()])
    }
}

#[tokio::test]
async fn a_singleton_cannot_depend_on_a_request_scoped_provider() {
    let error = bootstrap::<GreedyModule>().await.unwrap_err();
    match error {
        DiError::ScopeMismatch {
            provider_lifetime,
            dependency_lifetime,
            ..
        } => {
            assert_eq!(provider_lifetime, Lifetime::Singleton);
            assert_eq!(dependency_lifetime, Lifetime::Request);
        }
        other => panic!("expected ScopeMismatch, got {other}"),
    }
}

struct PerRequestModule;
impl Module for PerRequestModule {
    fn definition() -> ModuleDef {
        ModuleDef::new("PerRequestModule")
            .providers(scoped_providers())
            // A controller-like singleton that resolves the context per call.
            .providers([Provider::of::<Greedy>().with_request_dependencies([Token::of::<RequestContext>()])])
    }
}

#[tokio::test]
async fn singletons_may_declare_per_request_dependencies_for_call_time_resolution() {
    assert!(bootstrap::<PerRequestModule>().await.is_ok());
}

struct MissingModule;
impl Module for MissingModule {
    fn definition() -> ModuleDef {
        ModuleDef::new("MissingModule")
            .providers([Provider::of::<Greedy>().with_request_dependencies([Token::of::<RequestContext>()])])
    }
}

#[tokio::test]
async fn per_request_dependencies_must_still_exist() {
    let error = bootstrap::<MissingModule>().await.unwrap_err();
    assert!(matches!(error, DiError::MissingProvider { .. }), "{error}");
}
