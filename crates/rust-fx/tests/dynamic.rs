#![allow(missing_docs)]
//! The runtime-described provider: the seam other frameworks build on.

use std::any::TypeId;
use std::sync::{Arc, Mutex};

use fx::{App, DepKind, DepSpec, Key, Lifetime, Options, Provider, Resolved, load, store};

struct Base(u32);
struct Derived(u32);

/// A key assembled from runtime parts equals the typed one.
fn derived_key() -> Key {
    Key::from_parts(TypeId::of::<Derived>(), "Derived", None, false)
}

fn dynamic_derived(log: Arc<Mutex<Vec<&'static str>>>) -> Provider {
    Provider::dynamic(
        derived_key(),
        vec![DepSpec {
            key: Key::of::<Base>(),
            kind: DepKind::Required,
        }],
        "dynamic",
        Lifetime::Singleton,
        move |args, ctx| {
            let log = log.clone();
            let life = ctx.lifecycle.clone();
            Box::pin(async move {
                let Some(Resolved::One(base)) = args.into_iter().next() else {
                    return Err("expected one argument".into());
                };
                let base = load::<Base>(&base).ok_or("wrong type")?;
                life.on_start({
                    let log = log.clone();
                    move || async move { log.lock().unwrap().push("start") }
                });
                life.on_stop(move || async move { log.lock().unwrap().push("stop") });
                Ok(store(Arc::new(Derived(base.0 + 1))))
            })
        },
    )
}

#[test]
fn keys_from_parts_equal_typed_keys() {
    assert_eq!(derived_key(), Key::of::<Derived>());
    assert_eq!(derived_key().type_id(), TypeId::of::<Derived>());
    assert_eq!(derived_key().type_name(), "Derived");
}

#[tokio::test]
async fn a_dynamic_provider_resolves_dependencies_and_registers_hooks() {
    let log = Arc::default();
    let options = Options::new()
        .supply(Base(41))
        .constructor(dynamic_derived(Arc::clone(&log)));
    let mut app = App::new().options(options).eager();
    app.start().await.unwrap();
    // A typed consumer sees the dynamic value.
    assert_eq!(app.get::<Derived>().await.unwrap().0, 42);
    app.stop().await.unwrap();
    assert_eq!(*log.lock().unwrap(), ["start", "stop"]);
}

#[tokio::test]
async fn a_dynamic_provider_can_be_the_dependency_of_a_typed_one() {
    let options = Options::new()
        .supply(Base(1))
        .constructor(dynamic_derived(Arc::default()))
        .constructor(|d: Arc<Derived>| Arc::new(format!("derived is {}", d.0)));
    let mut app = App::new().options(options);
    app.start().await.unwrap();
    assert_eq!(*app.get::<String>().await.unwrap(), "derived is 2");
}

#[tokio::test]
async fn a_dynamic_provider_obeys_the_planner_like_any_other() {
    let options = Options::new()
        .supply(Derived(0))
        .constructor(dynamic_derived(Arc::default()))
        .supply(Base(1));
    let error = App::new().options(options).eager().start().await.unwrap_err();
    assert!(matches!(error, fx::Error::Duplicate { .. }), "{error}");
}
