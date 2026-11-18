//! The `fx::test` harness. Needs `--features test`.
#![cfg(feature = "test")]

use std::sync::Arc;

use fx::Lifecycle;

struct Config;

#[tokio::test]
async fn the_recorder_sees_construction_and_hooks_in_order() {
    let mut app = fx::test::App::new()
        .provide(|life: Lifecycle| {
            life.on_start(|| async {});
            life.on_stop(|| async {});
            Arc::new(Config)
        })
        .invoke(|_: Arc<Config>| {});
    let recorder = app.recorder();

    app.start().await.unwrap();
    app.stop().await.unwrap();

    let summary = recorder.summary();
    assert_eq!(summary.len(), 6, "{summary:?}");
    assert_eq!(summary[0], "provided Config");
    assert!(summary[1].starts_with("invoked "));
    assert!(summary[2].starts_with("start "));
    assert_eq!(summary[3], "started");
    assert!(summary[4].starts_with("stop "));
    assert_eq!(summary[5], "stopped");
}

#[tokio::test]
async fn replace_keeps_the_real_provider_out_of_the_test() {
    struct Real;
    let mut app = fx::test::App::new()
        .provide(|| -> Result<Arc<String>, std::io::Error> { Err(std::io::Error::other("would hit the network")) })
        .provide(|_: Arc<String>| Arc::new(Real))
        .replace(Arc::new(String::from("fake")));
    app.validate().unwrap();
    app.start().await.unwrap();
    assert_eq!(*app.get::<String>().await.unwrap(), "fake");
    assert!(app.get::<Real>().await.is_ok());
    app.stop().await.unwrap();
}
