//! Hand-written axum routes with the same protections and documentation as a controller.
//!
//! `AppBuilder::merge` mounts a raw [`Router`] and gives it the app-wide pipeline (scopes,
//! filters, prefix, app interceptors). A [`Mount`] adds what a controller declares with
//! attributes, as a builder:
//!
//! ```ignore
//! App::builder(AppModule)
//!     .mount(
//!         Mount::new(Router::new().route("/webhooks/stripe", post(stripe_webhook)))
//!             .interceptor(RateLimit::per_minute(120))
//!             .guard::<BearerAuth>()
//!             .document(
//!                 RouteDoc::post("/webhooks/stripe", "stripe_webhook")
//!                     .tagged("Webhooks")
//!                     .request_body(BodyDoc::of::<StripeEvent>())
//!                     .response(ResponseDoc::new(200, None)),
//!             ),
//!     )
//!     .build()
//!     .await?
//! ```
//!
//! ## Order
//!
//! The same as a controller: **guards, then interceptors, then the handler**, whatever order the
//! builder calls are written in. Among interceptors the last added is outermost, as with
//! `Service::interceptor`. A documented route is marked secured when the mount has a guard.
//!
//! ## Design
//!
//! A mount is data (a router factory, layers, docs) that the app builder consumes; it adds no
//! behaviour of its own, so the guard and interceptor semantics stay in `guard.rs` and
//! `interceptor.rs` (SRP, DRY).

use std::sync::Arc;

use axum::Router;
use purlin_core::{Container, DiError};

use crate::interceptor::intercepted_router;
use crate::openapi::RouteDoc;
use crate::{Guard, Interceptor, guarded_router};

type Build = Box<dyn FnOnce(&Container) -> Result<Router, DiError> + Send>;
type Layer = Box<dyn FnOnce(Router, &Container) -> Result<Router, DiError> + Send>;

/// A raw router plus the guards, interceptors and OpenAPI entries a controller would give it.
/// Pass it to `AppBuilder::mount`.
pub struct Mount {
    build: Build,
    guards: Vec<Layer>,
    interceptors: Vec<Layer>,
    docs: Vec<RouteDoc>,
    /// `Some(true)` while every guard added so far lets anonymous callers through.
    anonymous_ok: Option<bool>,
}

impl Mount {
    /// Mounts `router` as it is.
    pub fn new(router: Router) -> Self {
        Self::with(move |_| Ok(router))
    }

    /// Builds the router from the container, after the module graph, so handlers can be given
    /// providers as axum state. Providers must be in the root or a global module.
    pub fn with(build: impl FnOnce(&Container) -> Result<Router, DiError> + Send + 'static) -> Self {
        Self {
            build: Box::new(build),
            guards: Vec::new(),
            interceptors: Vec::new(),
            docs: Vec::new(),
            anonymous_ok: None,
        }
    }

    /// Runs guard `G` (resolved from the container, the same instance controllers use) before
    /// every route. Several guards all run, in the order added.
    pub fn guard<G: Guard>(mut self) -> Self {
        self.anonymous_ok = Some(self.anonymous_ok.unwrap_or(true) && G::ANONYMOUS_OK);
        self.guards.push(Box::new(|router, container| {
            Ok(guarded_router(router, container.resolve::<G>()?))
        }));
        self
    }

    /// Wraps every route in `interceptor`.
    pub fn interceptor<I: Interceptor>(mut self, interceptor: I) -> Self {
        let interceptor = Arc::new(interceptor);
        self.interceptors
            .push(Box::new(move |router, _| Ok(intercepted_router(router, interceptor))));
        self
    }

    /// Documents a route in the OpenAPI output. Paths are written as the router has them, without
    /// the global prefix, which is added like for controllers.
    pub fn document(mut self, doc: RouteDoc) -> Self {
        self.docs.push(doc);
        self
    }

    /// The router factory, with guards and interceptors applied, and the docs.
    pub(crate) fn into_parts(self) -> (Build, Vec<RouteDoc>) {
        let Self {
            build,
            guards,
            interceptors,
            docs,
            anonymous_ok,
        } = self;
        let docs = docs
            .into_iter()
            .map(|doc| match anonymous_ok {
                Some(anonymous) => doc.secured(true).anonymous_ok(anonymous),
                None => doc,
            })
            .collect();
        let factory: Build = Box::new(move |container| {
            let mut router = build(container)?;
            // Innermost first: interceptors (last added outermost), then guards (last added
            // outermost, so the first one added runs first once reversed).
            for layer in interceptors {
                router = layer(router, container)?;
            }
            for layer in guards.into_iter().rev() {
                router = layer(router, container)?;
            }
            Ok(router)
        });
        (factory, docs)
    }
}
