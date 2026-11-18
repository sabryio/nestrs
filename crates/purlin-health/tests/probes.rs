//! Black-box probe tests through the real macros and module graph.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use purlin::http::axum::http::StatusCode;
use purlin::prelude::*;
use purlin_health::{HealthConfig, HealthIndicator, HealthIndicatorPort, HealthModule, Unhealthy};
use purlin_testing::TestApp;
use serde_json::{Value, json};

/// Flags the tests flip to make indicators fail.
#[derive(Default)]
#[injectable]
struct Switches {
    #[inject(default)]
    database_down: AtomicBool,
    #[inject(default)]
    cache_down: AtomicBool,
}

#[injectable(contributes = HealthIndicatorPort)]
struct Database {
    switches: Arc<Switches>,
}
#[async_trait]
impl HealthIndicator for Database {
    fn name(&self) -> &str {
        "database"
    }
    async fn check(&self) -> Result<(), Unhealthy> {
        match self.switches.database_down.load(Ordering::SeqCst) {
            true => Err(Unhealthy::new("connection refused")),
            false => Ok(()),
        }
    }
}

#[injectable(contributes = HealthIndicatorPort)]
struct Cache {
    switches: Arc<Switches>,
}
#[async_trait]
impl HealthIndicator for Cache {
    fn name(&self) -> &str {
        "cache"
    }
    fn critical(&self) -> bool {
        false
    }
    async fn check(&self) -> Result<(), Unhealthy> {
        match self.switches.cache_down.load(Ordering::SeqCst) {
            true => Err(Unhealthy::new("evicted everything")),
            false => Ok(()),
        }
    }
}

/// An optional dependency that is not configured here: critical and broken, but disabled.
#[injectable(contributes = HealthIndicatorPort)]
struct NotConfigured;
#[async_trait]
impl HealthIndicator for NotConfigured {
    fn name(&self) -> &str {
        "not-configured"
    }
    fn enabled(&self) -> bool {
        false
    }
    async fn check(&self) -> Result<(), Unhealthy> {
        Err(Unhealthy::new("would fail if it ran"))
    }
}

/// Each feature module contributes its own check; the health module knows neither.
#[module(providers = [Switches], exports = [Switches], global = true)]
struct SwitchesModule;
#[module(providers = [Database, NotConfigured])]
struct DatabaseModule;
#[module(providers = [Cache])]
struct CacheModule;

struct AppModule;
impl Module for AppModule {
    fn definition() -> ModuleDef {
        ModuleDef::new("AppModule").imports([
            HealthModule::for_root(HealthConfig {
                timeout: Duration::from_millis(50),
                expose_details: true,
            }),
            ModuleRef::of::<SwitchesModule>(),
            ModuleRef::of::<DatabaseModule>(),
            ModuleRef::of::<CacheModule>(),
        ])
    }
}

#[tokio::test]
async fn liveness_never_looks_at_dependencies() {
    let app = TestApp::start(AppModule).await;
    app.resolve::<Switches>().database_down.store(true, Ordering::SeqCst);
    let body: Value = app
        .client()
        .get("/health/live")
        .send()
        .await
        .assert_status(StatusCode::OK)
        .json();
    assert_eq!(body, json!({ "status": "ok" }));
}

#[tokio::test]
async fn ready_lists_every_contributed_check() {
    let app = TestApp::start(AppModule).await;
    let body: Value = app
        .client()
        .get("/health/ready")
        .send()
        .await
        .assert_status(StatusCode::OK)
        .json();
    assert_eq!(body["status"], "ok");
    let names: Vec<_> = body["checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(names.len(), 2);
    assert!(names.contains(&"database") && names.contains(&"cache"));
    assert!(body["checks"][0]["status"] == "up" && body["checks"][0].get("error").is_none());
}

#[tokio::test]
async fn a_disabled_indicator_is_neither_run_nor_reported() {
    let app = TestApp::start(AppModule).await;
    let body: Value = app
        .client()
        .get("/health/ready")
        .send()
        .await
        .assert_status(StatusCode::OK)
        .json();
    let names: Vec<_> = body["checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert!(!names.contains(&"not-configured"), "{names:?}");
}

#[tokio::test]
async fn a_failing_critical_check_makes_the_service_not_ready() {
    let app = TestApp::start(AppModule).await;
    app.resolve::<Switches>().database_down.store(true, Ordering::SeqCst);

    let response = app
        .client()
        .get("/health/ready")
        .send()
        .await
        .assert_status(StatusCode::SERVICE_UNAVAILABLE);
    let body: Value = response.json();
    assert_eq!(body["error"]["code"], "unavailable");
    let checks = body["error"]["details"]["checks"].as_array().unwrap();
    let database = checks.iter().find(|c| c["name"] == "database").unwrap();
    assert_eq!(database["status"], "down");
    assert_eq!(database["error"], "connection refused");
    let cache = checks.iter().find(|c| c["name"] == "cache").unwrap();
    assert_eq!(cache["status"], "up", "one failure does not hide the others");
}

#[tokio::test]
async fn a_failing_non_critical_check_only_degrades() {
    let app = TestApp::start(AppModule).await;
    app.resolve::<Switches>().cache_down.store(true, Ordering::SeqCst);
    let body: Value = app
        .client()
        .get("/health/ready")
        .send()
        .await
        .assert_status(StatusCode::OK)
        .json();
    assert_eq!(body["status"], "degraded");
}

#[tokio::test]
async fn readiness_recovers_when_the_dependency_does() {
    let app = TestApp::start(AppModule).await;
    let switches = app.resolve::<Switches>();
    switches.database_down.store(true, Ordering::SeqCst);
    app.client()
        .get("/health/ready")
        .send()
        .await
        .assert_status(StatusCode::SERVICE_UNAVAILABLE);
    switches.database_down.store(false, Ordering::SeqCst);
    app.client()
        .get("/health/ready")
        .send()
        .await
        .assert_status(StatusCode::OK);
}

#[tokio::test]
async fn shutting_down_flips_readiness_but_not_liveness() {
    let app = TestApp::start(AppModule).await;
    app.shutdown().await;
    let body: Value = app
        .client()
        .get("/health/ready")
        .send()
        .await
        .assert_status(StatusCode::SERVICE_UNAVAILABLE)
        .json();
    assert_eq!(body["error"]["details"]["status"], "draining");
    app.client()
        .get("/health/live")
        .send()
        .await
        .assert_status(StatusCode::OK);
}

// ------------------------------------------------ misbehaving indicators

#[injectable(contributes = HealthIndicatorPort)]
struct Hangs;
#[async_trait]
impl HealthIndicator for Hangs {
    fn name(&self) -> &str {
        "hangs"
    }
    async fn check(&self) -> Result<(), Unhealthy> {
        tokio::time::sleep(Duration::from_secs(60)).await;
        Ok(())
    }
}

#[injectable(contributes = HealthIndicatorPort)]
struct Explodes;
#[async_trait]
impl HealthIndicator for Explodes {
    fn name(&self) -> &str {
        "explodes"
    }
    async fn check(&self) -> Result<(), Unhealthy> {
        panic!("boom")
    }
}

#[module(providers = [Hangs, Explodes])]
struct BadActors;

/// Apps whose only contributors are the misbehaving ones.
struct Verbose;
impl Module for Verbose {
    fn definition() -> ModuleDef {
        ModuleDef::new("Verbose").imports([
            HealthModule::for_root(HealthConfig::default()),
            ModuleRef::of::<BadActors>(),
        ])
    }
}
struct Quiet;
impl Module for Quiet {
    fn definition() -> ModuleDef {
        let config = HealthConfig {
            expose_details: false,
            ..HealthConfig::default()
        };
        ModuleDef::new("Quiet").imports([HealthModule::for_root(config), ModuleRef::of::<BadActors>()])
    }
}

#[tokio::test(start_paused = true)]
async fn slow_and_panicking_indicators_fail_their_own_check_only() {
    let app = TestApp::start(Verbose).await;
    let started = tokio::time::Instant::now();
    let body: Value = app
        .client()
        .get("/health/ready")
        .send()
        .await
        .assert_status(StatusCode::SERVICE_UNAVAILABLE)
        .json();
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "the hanging check was cut off at the 2 s timeout, not 60 s"
    );

    let checks = body["error"]["details"]["checks"].as_array().unwrap();
    let error_of = |name: &str| {
        checks.iter().find(|c| c["name"] == name).unwrap()["error"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    assert!(error_of("hangs").starts_with("timed out after"));
    assert_eq!(error_of("explodes"), "the check panicked");
}

#[tokio::test(start_paused = true)]
async fn details_can_be_hidden() {
    let app = TestApp::start(Quiet).await;
    let body: Value = app.client().get("/health/ready").send().await.json();
    let checks = body["error"]["details"]["checks"].as_array().unwrap();
    assert!(checks.iter().all(|c| c.get("error").is_none()));
}

#[tokio::test]
async fn with_no_indicators_at_all_ready_means_up() {
    #[module(imports = [HealthModule])]
    struct Bare;
    let app = TestApp::start(Bare).await;
    let body: Value = app
        .client()
        .get("/health/ready")
        .send()
        .await
        .assert_status(StatusCode::OK)
        .json();
    assert_eq!(body, json!({ "status": "ok", "checks": [] }));
}
