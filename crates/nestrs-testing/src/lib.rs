//! Test harness for nestrs applications.
//!
//! [`TestApp`] boots your real root module in-process, optionally with providers
//! swapped for fakes, and gives you a [`TestClient`] that sends requests straight
//! into the router: no sockets, no ports, no sleeps.
//!
//! ```ignore
//! #[tokio::test]
//! async fn creating_a_user_requires_a_token() {
//!     let app = TestApp::builder(AppModule)
//!         .override_value(AppConfig { api_token: "t".into(), ..Default::default() })
//!         .override_instance::<dyn UserRepository>(Arc::new(FakeUsers::default()))
//!         .start()
//!         .await;
//!     let client = app.client();
//!
//!     client.post("/users").json(&json!({ "name": "Ada", "email": "ada@example.com" }))
//!         .send().await
//!         .assert_status(StatusCode::UNAUTHORIZED);
//!
//!     let user: Value = client.post("/users").bearer("t")
//!         .json(&json!({ "name": "Ada", "email": "ada@example.com" }))
//!         .send().await
//!         .assert_status(StatusCode::CREATED)
//!         .json();
//! }
//! ```

#![forbid(unsafe_code)]

mod client;

pub use client::{RequestBuilder, TestClient, TestResponse};

use std::sync::Arc;

use nestrs_core::{Container, Dependency, DiError, Module, Overrides, Provider};
use nestrs_http::axum::Router;
use nestrs_http::{App, AppBuilder, AppError, ExceptionFilter};

/// Configures a [`TestApp`]; obtained from [`TestApp::builder`].
pub struct TestAppBuilder<M: Module> {
    inner: AppBuilder<M>,
    prefix: Option<String>,
}

impl<M: Module> TestAppBuilder<M> {
    /// Replaces the provider for `provider`'s type (see [`Overrides`] for the rules:
    /// the last override wins, an override nobody provides is an error).
    pub fn override_provider(mut self, provider: Provider) -> Self {
        self.inner = self.inner.override_provider(provider);
        self
    }

    /// Overrides a concrete type with a ready-made value.
    pub fn override_value<T: Send + Sync + 'static>(self, value: T) -> Self {
        self.override_provider(Provider::value(value))
    }

    /// Overrides an interface (`dyn Trait`) with a fake.
    pub fn override_instance<T: ?Sized + Send + Sync + 'static>(self, instance: Arc<T>) -> Self {
        self.override_provider(Provider::instance::<T>(instance))
    }

    /// Replaces a port (or any `Arc<T>` handle) with a fake, without naming `dyn`:
    ///
    /// ```ignore
    /// .mock::<UserRepositoryPort>(Arc::new(FakeUsers::default()))
    /// ```
    ///
    /// The argument is coerced to the handle type, so passing an `Arc` of your fake
    /// is enough. Keep a clone of the `Arc` to assert on it later.
    pub fn mock<P: Dependency>(self, handle: P) -> Self {
        self.override_provider(Provider::instance::<P::Target>(handle.into_shared()))
    }

    /// Applies a prepared set of overrides.
    pub fn overrides(mut self, overrides: Overrides) -> Self {
        self.inner = self.inner.overrides(overrides);
        self
    }

    /// Registers an exception filter, as `App::builder(..).filter` does.
    pub fn filter<F: ExceptionFilter>(mut self) -> Self {
        self.inner = self.inner.filter::<F>();
        self
    }

    /// Mounts a hand-written axum router next to the controllers, as
    /// `App::builder(..).merge` does.
    pub fn merge(mut self, router: Router) -> Self {
        self.inner = self.inner.merge(router);
        self
    }

    /// Like [`merge`](Self::merge), with the router built from the container.
    pub fn merge_with(mut self, build: impl FnOnce(&Container) -> Result<Router, DiError> + Send + 'static) -> Self {
        self.inner = self.inner.merge_with(build);
        self
    }

    /// Mounts the app under `prefix`, like `App::global_prefix`; the client then
    /// needs the prefix in its paths.
    pub fn global_prefix(mut self, prefix: &str) -> Self {
        self.prefix = Some(prefix.to_owned());
        self
    }

    /// Builds the app, returning the startup error instead of panicking. Use this to
    /// assert that a bad configuration or graph is rejected.
    pub async fn try_start(self) -> Result<TestApp, AppError> {
        let mut app = self.inner.build().await?;
        if let Some(prefix) = self.prefix {
            app = app.global_prefix(&prefix);
        }
        let router = app.router();
        Ok(TestApp { app, router })
    }

    /// Builds the app, panicking with the startup error if it fails.
    pub async fn start(self) -> TestApp {
        match self.try_start().await {
            Ok(app) => app,
            Err(error) => panic!("the application under test failed to start: {error}"),
        }
    }
}

/// A running application under test.
pub struct TestApp {
    app: App,
    router: Router,
}

impl TestApp {
    /// Starts configuring a test app for the root module `M`.
    pub fn builder<M: Module>(root: M) -> TestAppBuilder<M> {
        TestAppBuilder {
            inner: App::builder(root),
            prefix: None,
        }
    }

    /// Boots `root` as it is, panicking if startup fails.
    pub async fn start<M: Module>(root: M) -> Self {
        Self::builder(root).start().await
    }

    /// A client that sends requests into this app.
    pub fn client(&self) -> TestClient {
        TestClient::new(self.router.clone())
    }

    /// The built providers, e.g. to inspect a fake after the test acted.
    pub fn container(&self) -> &Container {
        self.app.container()
    }

    /// Resolves a provider, panicking if it does not exist.
    pub fn resolve<T: ?Sized + Send + Sync + 'static>(&self) -> Arc<T> {
        match self.app.container().resolve::<T>() {
            Ok(instance) => instance,
            Err(error) => panic!("cannot resolve a provider from the application under test: {error}"),
        }
    }

    /// Runs the shutdown hooks and returns their errors (empty on success).
    pub async fn shutdown(&self) -> Vec<DiError> {
        self.app.shutdown().await
    }

    /// The underlying app, for features the harness does not wrap (`openapi`, ..).
    pub fn app(&self) -> &App {
        &self.app
    }
}
