//! A purlin module graph hosted inside an fx app: one lifecycle, one shutdown.

#![allow(missing_docs)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use purlin::hosting::{Hosted, hosted};
use purlin::{fx, injectable, lifecycle, module};

static OPEN: AtomicBool = AtomicBool::new(false);

#[injectable]
pub struct Pool;

#[lifecycle]
impl Pool {
    #[on_init]
    async fn connect(&self) -> Result<(), String> {
        OPEN.store(true, Ordering::SeqCst);
        Ok(())
    }

    #[on_destroy]
    async fn close(&self) -> Result<(), String> {
        OPEN.store(false, Ordering::SeqCst);
        Ok(())
    }
}

#[injectable]
pub struct Repo {
    pool: Arc<Pool>,
}

impl Repo {
    fn pool(&self) -> &Pool {
        &self.pool
    }
}

#[module(providers = [Pool, Repo], exports = [Repo])]
pub struct DataModule;

#[module(imports = [DataModule])]
pub struct AppModule;

#[tokio::test]
async fn start_boots_the_graph_and_stop_runs_its_destroy_hooks() {
    let mut app = fx::App::new().options(hosted::<AppModule>());
    app.start().await.expect("starts");
    assert!(OPEN.load(Ordering::SeqCst), "on_init ran during fx start");

    let host = app.get::<Hosted>().await.expect("hosted is resolvable");
    assert_eq!(host.modules(), ["DataModule", "AppModule"]);
    let repo = host
        .resolve::<Repo>()
        .expect("singletons are reachable through the host");
    let _ = repo.pool();

    app.stop().await.expect("stops");
    assert!(!OPEN.load(Ordering::SeqCst), "on_destroy ran during fx stop");
}

#[tokio::test]
async fn other_fx_components_can_depend_on_the_hosted_graph() {
    #[derive(fx::Component)]
    struct Probe(Arc<Hosted>);

    // No lifecycle hooks here: tests run in parallel and `OPEN` belongs to the first one.
    #[injectable]
    pub struct Plain;
    #[module(providers = [Plain])]
    pub struct PlainModule;
    #[module(imports = [PlainModule])]
    pub struct PlainApp;

    let mut app = fx::App::new().options(
        hosted::<PlainApp>()
            .constructor(|host: Arc<Hosted>| Probe(host))
            .invoke(|probe: Arc<Probe>| assert_eq!(probe.0.modules().len(), 2)),
    );
    app.start().await.expect("starts");
    app.stop().await.expect("stops");
}

#[tokio::test]
async fn a_broken_graph_fails_the_fx_start_phase() {
    #[injectable]
    pub struct Needy {
        _missing: Arc<Absent>,
    }
    pub struct Absent;
    #[module(providers = [Needy])]
    pub struct Broken;

    let error = fx::App::new()
        .options(hosted::<Broken>())
        .start()
        .await
        .expect_err("must fail");
    assert!(error.to_string().contains("Absent"), "{error}");
}
