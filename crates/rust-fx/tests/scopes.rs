#![allow(missing_docs)]
//! Request-scoped and transient values.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use fx::{App, Error, Lifetime, Options, constructor};

#[derive(Debug)]
struct Pool;
#[derive(Debug)]
struct RequestId(u32);
#[derive(Debug)]
struct Handler {
    id: Arc<RequestId>,
    pool: Arc<Pool>,
}
struct Ticket(usize);

static BUILT: AtomicUsize = AtomicUsize::new(0);

fn pool() -> Arc<Pool> {
    Arc::new(Pool)
}

fn handler(id: Arc<RequestId>, pool: Arc<Pool>) -> Arc<Handler> {
    Arc::new(Handler { id, pool })
}

fn base() -> Options {
    Options::new()
        .seeded::<RequestId>()
        .constructor(pool)
        .constructor(constructor(handler).request())
}

async fn started(options: Options) -> (App, fx::Scopes) {
    let mut app = App::new().options(options);
    app.start().await.unwrap();
    let scopes = app.scopes().unwrap();
    (app, scopes)
}

#[tokio::test]
async fn request_scoped_values_are_built_once_per_scope() {
    // `Pool` is a lazy singleton nobody invokes: starting must still build it.
    let (_app, scopes) = started(base()).await;
    assert_eq!(scopes.len(), 2);

    let one = scopes.open();
    one.seed(Arc::new(RequestId(1))).unwrap();
    let (a, b) = (
        one.resolve::<Handler>().await.unwrap(),
        one.resolve::<Handler>().await.unwrap(),
    );
    assert!(Arc::ptr_eq(&a, &b));
    assert_eq!(a.id.0, 1);

    let two = scopes.open();
    two.seed(Arc::new(RequestId(2))).unwrap();
    let c = two.resolve::<Handler>().await.unwrap();
    assert!(!Arc::ptr_eq(&a, &c));
    assert_eq!(c.id.0, 2);
    assert!(Arc::ptr_eq(&a.pool, &c.pool), "singletons are shared across scopes");
}

#[tokio::test]
async fn transient_values_are_built_on_every_resolution() {
    BUILT.store(0, Ordering::SeqCst);
    let options =
        Options::new().constructor(constructor(|| Arc::new(Ticket(BUILT.fetch_add(1, Ordering::SeqCst)))).transient());
    let (_app, scopes) = started(options).await;
    let scope = scopes.open();
    let first = scope.resolve::<Ticket>().await.unwrap();
    let second = scope.resolve::<Ticket>().await.unwrap();
    assert_ne!(first.0, second.0);
}

#[tokio::test]
async fn an_unseeded_value_is_a_clear_error() {
    let (_app, scopes) = started(base()).await;
    let error = scopes.open().resolve::<Handler>().await.unwrap_err();
    assert!(error.to_string().contains("seeded"), "{error}");
}

#[tokio::test]
async fn a_singleton_cannot_depend_on_a_scoped_value() {
    let options = base().constructor(|h: Arc<Handler>| Arc::new(Ticket(h.id.0 as usize)));
    let error = App::new().options(options).start().await.unwrap_err();
    assert!(matches!(error, Error::ScopeMismatch { .. }), "{error}");
    assert!(error.to_string().contains("request-scoped"), "{error}");
}

#[tokio::test]
async fn an_invoke_cannot_depend_on_a_scoped_value() {
    let options = base().invoke(|_: Arc<Handler>| {});
    let error = App::new().options(options).start().await.unwrap_err();
    assert!(matches!(error, Error::ScopeMismatch { .. }), "{error}");
}

#[tokio::test]
async fn scoped_values_may_depend_on_transient_values() {
    let options = Options::new()
        .constructor(constructor(|| Arc::new(Ticket(7))).transient())
        .constructor(constructor(|t: Arc<Ticket>| Arc::new(RequestId(t.0 as u32))).request());
    let (_app, scopes) = started(options).await;
    assert_eq!(scopes.open().resolve::<RequestId>().await.unwrap().0, 7);
}

#[tokio::test]
async fn scopes_need_a_started_app() {
    assert!(matches!(App::new().scopes(), Err(Error::NotStarted)));
}

#[tokio::test]
async fn seeding_something_that_is_not_request_scoped_is_an_error() {
    let (_app, scopes) = started(base()).await;
    assert!(scopes.open().seed(Arc::new(Pool)).is_err());
    assert_eq!(fx::Provider::from(constructor(pool)).lifetime(), Lifetime::Singleton);
}
