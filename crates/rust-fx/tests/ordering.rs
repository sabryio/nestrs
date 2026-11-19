#![allow(missing_docs)]
//! Start/stop ordering, pinned. These are the guarantees a framework built on fx relies on,
//! and they mirror what `purlin-core`'s own lifecycle tests assert about its bootstrap.

use std::sync::{Arc, Mutex};

use fx::{App, Error, Options, Phase, constructor};

type Log = Arc<Mutex<Vec<String>>>;

struct Db;
struct Cache(#[allow(dead_code)] Arc<Db>);
struct Idle;

fn note(log: &Log, line: &str) {
    log.lock().unwrap().push(line.to_owned());
}

fn lines(log: &Log) -> Vec<String> {
    log.lock().unwrap().clone()
}

/// `Cache` is declared before `Db` on purpose: declaration order must not matter.
fn graph(log: &Log) -> Options {
    let (a, b, c, d) = (log.clone(), log.clone(), log.clone(), log.clone());
    Options::new()
        .constructor(
            constructor(|db: Arc<Db>| Arc::new(Cache(db)))
                .on_start(move |_| {
                    let log = a.clone();
                    async move { note(&log, "start cache") }
                })
                .on_stop(move |_| {
                    let log = b.clone();
                    async move { note(&log, "stop cache") }
                }),
        )
        .constructor(
            constructor(|| Arc::new(Db))
                .on_start(move |_| {
                    let log = c.clone();
                    async move { note(&log, "start db") }
                })
                .on_stop(move |_| {
                    let log = d.clone();
                    async move { note(&log, "stop db") }
                }),
        )
}

#[tokio::test]
async fn start_runs_dependencies_first_and_stop_runs_in_reverse() {
    let log = Log::default();
    let mut app = App::new().options(graph(&log)).eager();
    app.start().await.unwrap();
    assert_eq!(lines(&log), ["start db", "start cache"]);
    app.stop().await.unwrap();
    assert_eq!(
        lines(&log),
        ["start db", "start cache", "stop cache", "stop db"],
        "dependents stop before what they depend on"
    );
}

#[tokio::test]
async fn lazy_start_skips_what_nobody_needs_and_eager_start_does_not() {
    let log = Log::default();
    let hooked = |log: &Log| {
        let log = log.clone();
        Options::new().constructor(constructor(|| Arc::new(Idle)).on_start(move |_| {
            let log = log.clone();
            async move { note(&log, "start idle") }
        }))
    };

    let mut lazy = App::new().options(hooked(&log));
    lazy.start().await.unwrap();
    assert!(lines(&log).is_empty(), "nothing injects Idle, so it is never built");

    let mut eager = App::new().options(hooked(&log)).eager();
    eager.start().await.unwrap();
    assert_eq!(lines(&log), ["start idle"]);
}

#[tokio::test]
async fn a_failing_start_unwinds_what_had_started_but_not_the_failing_hook() {
    let log = Log::default();
    let fail_log = log.clone();
    let options = graph(&log).constructor(
        constructor(|_: Arc<Db>| Arc::new(Idle))
            .on_start(|_| async { Err::<(), _>(std::io::Error::other("disk full")) })
            .on_stop(move |_| {
                let log = fail_log.clone();
                async move { note(&log, "stop idle") }
            }),
    );
    let mut app = App::new().options(options).eager();
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
    assert!(error.to_string().contains("disk full"), "{error}");
    assert_eq!(
        lines(&log),
        ["start db", "start cache", "stop cache", "stop db"],
        "the failing hook's own stop never runs; what had started is torn down in reverse"
    );
    assert!(!app.is_running());
}

#[tokio::test]
async fn one_failing_stop_hook_does_not_stop_the_others() {
    let log = Log::default();
    let options = graph(&log)
        .constructor(constructor(|| Arc::new(Idle)).on_stop(|_| async { Err::<(), _>(std::io::Error::other("boom")) }));
    let mut app = App::new().options(options).eager();
    app.start().await.unwrap();
    let error = app.stop().await.unwrap_err();
    assert!(error.to_string().contains("boom"), "{error}");
    assert!(
        lines(&log).ends_with(&["stop cache".to_owned(), "stop db".to_owned()]),
        "{:?}",
        lines(&log)
    );
}

#[tokio::test]
async fn a_second_stop_is_reported_not_repeated() {
    let log = Log::default();
    let mut app = App::new().options(graph(&log)).eager();
    app.start().await.unwrap();
    app.stop().await.unwrap();
    assert!(matches!(app.stop().await, Err(Error::NotStarted)));
    assert_eq!(lines(&log).iter().filter(|l| *l == "stop db").count(), 1);
}
