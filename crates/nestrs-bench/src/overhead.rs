//! The same endpoint, with one more feature in each variant.

use std::sync::Arc;
use std::time::Duration;

use nestrs::http::axum::Router;
use nestrs::http::axum::extract::State;
use nestrs::http::axum::routing::get;
use nestrs::http::{BearerAuth, StaticTokens};
use nestrs::prelude::*;
use nestrs_interceptors::{RequestId, SecurityHeaders};

/// The "business logic" every variant calls.
#[injectable]
pub struct Greeter;

impl Greeter {
    /// The response body.
    pub fn greet(&self) -> &'static str {
        "hello"
    }
}

// ── nestrs variants ─────────────────────────────────────────────────────────

/// Singleton injected as a controller field.
#[injectable]
pub struct Bare {
    greeter: Arc<Greeter>,
}

#[controller("/bare")]
impl Bare {
    #[get("/hello")]
    async fn hello(&self) -> &'static str {
        self.greeter.greet()
    }
}

/// Singleton resolved per request through `Inject<T>`.
#[injectable]
pub struct PerRequestInject;

#[controller("/inject")]
impl PerRequestInject {
    #[get("/hello")]
    async fn hello(&self, Inject(greeter): Inject<Greeter>) -> &'static str {
        greeter.greet()
    }
}

/// Bearer authentication in front of the handler.
#[injectable]
pub struct Guarded {
    greeter: Arc<Greeter>,
}

#[controller("/guarded")]
#[guard(BearerAuth)]
impl Guarded {
    #[get("/hello")]
    async fn hello(&self) -> &'static str {
        self.greeter.greet()
    }
}

/// Three route-level interceptors.
#[injectable]
pub struct Intercepted {
    greeter: Arc<Greeter>,
}

#[controller("/intercepted")]
#[interceptor(SecurityHeaders::default())]
#[interceptor(RequestId::default())]
#[interceptor(Timeout::new(Duration::from_secs(5)))]
impl Intercepted {
    #[get("/hello")]
    async fn hello(&self) -> &'static str {
        self.greeter.greet()
    }
}

/// Guard + roles + three interceptors + rate limit: a realistic protected route.
#[injectable]
pub struct Full {
    greeter: Arc<Greeter>,
}

#[controller("/full")]
#[guard(BearerAuth)]
#[interceptor(SecurityHeaders::default())]
#[interceptor(RequestId::default())]
#[interceptor(Timeout::new(Duration::from_secs(5)))]
impl Full {
    #[get("/hello")]
    #[roles("member")]
    #[interceptor(RateLimit::per_second(1_000_000_000))]
    async fn hello(&self) -> &'static str {
        self.greeter.greet()
    }
}

/// All the nestrs variants in one app.
#[module(
    providers = [
        Greeter,
        StaticTokens::new().token("t", Principal::new("ann").with_roles(["member"])).provider(),
        BearerAuth,
    ],
    controllers = [Bare, PerRequestInject, Guarded, Intercepted, Full],
)]
pub struct OverheadModule;

/// The nestrs app, as a router.
pub async fn nestrs_router() -> Router {
    App::create(OverheadModule)
        .await
        .expect("the benchmark app builds")
        .into_router()
}

// ── the baseline ────────────────────────────────────────────────────────────

/// The baseline: plain axum, state shared through `State`.
pub fn axum_router() -> Router {
    async fn hello(State(greeter): State<Arc<Greeter>>) -> &'static str {
        greeter.greet()
    }
    Router::new()
        .route("/axum/hello", get(hello))
        .with_state(Arc::new(Greeter))
}

/// The baseline plus three pass-through `from_fn` middlewares: roughly the always-on
/// layers of a nestrs `App` (request scopes, exception filters, shutdown signal). It
/// shows how much of nestrs's fixed cost is simply "three axum middlewares".
pub fn axum_layered_router() -> Router {
    use nestrs::http::axum::extract::Request;
    use nestrs::http::axum::middleware::{self, Next};
    use nestrs::http::axum::response::Response;

    async fn pass(request: Request, next: Next) -> Response {
        next.run(request).await
    }
    axum_router()
        .layer(middleware::from_fn(pass))
        .layer(middleware::from_fn(pass))
        .layer(middleware::from_fn(pass))
}

/// `(label, path)` of every row, in the order they are reported.
pub const SCENARIOS: &[(&str, &str)] = &[
    ("axum (baseline)", "/axum/hello"),
    ("axum + 3 pass-through middlewares", "/axum/hello#layered"),
    ("nestrs: controller", "/bare/hello"),
    ("nestrs: + Inject<T> argument", "/inject/hello"),
    ("nestrs: + bearer guard", "/guarded/hello"),
    ("nestrs: + 3 interceptors", "/intercepted/hello"),
    ("nestrs: guard + role + 3 interceptors + rate limit", "/full/hello"),
];
