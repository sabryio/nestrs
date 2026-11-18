use std::convert::Infallible;
use std::fmt;
use std::sync::Arc;

use axum::Router;
use axum::extract::Request;
use axum::http::header::CONTENT_TYPE;
use axum::response::Html;
use axum::response::IntoResponse;
use axum::routing::Route;
use axum::routing::get;
use std::marker::PhantomData;

use purlin_core::{Container, DiError, Module, Overrides, Provider, Shutdown, bootstrap_with};

use crate::filter::{ErasedFilter, with_filters};
use crate::openapi::{self, ApiInfo, RouteDoc};
use crate::scope::with_request_scopes;
use tokio::net::{TcpListener, ToSocketAddrs};
use tower::{Layer, Service};

use crate::{ExceptionFilter, HttpError, HttpTransport};

/// Everything that can stop an [`App`] from starting.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// The module graph is invalid or a provider failed to build.
    #[error(transparent)]
    Di(#[from] DiError),
    /// Binding the socket or serving failed.
    #[error("server error: {0}")]
    Io(#[from] std::io::Error),
}

/// A bootstrapped application: a built container plus the assembled router.
pub struct App {
    router: Router,
    container: Arc<Container>,
    modules: Vec<&'static str>,
    shutdown: Shutdown,
    docs: Vec<RouteDoc>,
    prefix: String,
    /// Flips to `true` when the app stops; open SSE streams end with `close`.
    stopping: tokio::sync::watch::Sender<bool>,
}

/// Configures an [`App`] before it is built; obtained from [`App::builder`].
///
/// Its main use is tests: swap real adapters for fakes without touching modules.
///
/// ```ignore
/// let fake = Arc::new(InMemoryUserRepository::default());
/// let app = App::builder(AppModule)
///     .override_provider(Provider::instance::<dyn UserRepository>(fake.clone()))
///     .build()
///     .await?;
/// ```
pub struct AppBuilder<M: Module> {
    overrides: Overrides,
    filters: Vec<FilterResolver>,
    merged: Vec<RouterFactory>,
    mounted_docs: Vec<RouteDoc>,
    root: PhantomData<M>,
}

/// Builds a merged router once the container exists.
type RouterFactory = Box<dyn FnOnce(&Container) -> Result<Router, DiError> + Send>;

/// Resolves one registered exception filter from the built container.
type FilterResolver = Box<dyn FnOnce(&Container) -> Result<ErasedFilter, DiError> + Send>;

impl<M: Module> AppBuilder<M> {
    /// Replaces the provider the graph has for `provider`'s type; see [`Overrides`]
    /// for the exact rules (unknown targets are an error, last override wins).
    pub fn override_provider(mut self, provider: Provider) -> Self {
        self.overrides = self.overrides.provide(provider);
        self
    }

    /// Applies a prepared set of overrides.
    pub fn overrides(mut self, overrides: Overrides) -> Self {
        self.overrides = self.overrides.extend(overrides);
        self
    }

    /// Registers an exception filter. `F` must be provided by some module (visible
    /// to nobody in particular: filters are app-wide, so provide it in the root or a
    /// global module). Filters run in registration order; the first to return a
    /// response wins.
    pub fn filter<F: ExceptionFilter>(mut self) -> Self {
        self.filters.push(Box::new(|container| {
            container.resolve::<F>().map(ErasedFilter::new::<F>)
        }));
        self
    }

    /// Mounts a hand-written axum router next to the controllers: the escape hatch for
    /// code that needs raw axum (a webhook, a WebSocket upgrade, a third-party router, or
    /// real axum `State`).
    ///
    /// ```ignore
    /// let webhooks = Router::new()
    ///     .route("/webhooks/stripe", post(stripe_webhook))
    ///     .with_state(StripeState::new());          // axum state stays inside this router
    /// App::builder(AppModule).merge(webhooks).build().await?
    /// ```
    ///
    /// Merged here, before `build`, the routes join the same pipeline as controller routes:
    ///
    /// - request scopes, so handlers may take `Inject<T>`;
    /// - exception filters, for any `HttpError` they return;
    /// - everything added to the built [`App`]: `global_prefix`, `interceptor`, `layer`,
    ///   the shutdown signal for streams.
    ///
    /// What they do **not** get: `#[guard]`, `#[roles]`, route-level `#[interceptor]` (those are
    /// controller attributes; wrap the router in your own `route_layer`, or use `App::interceptor`
    /// for something app-wide, or use [`mount`](Self::mount), which adds them with a builder), and
    /// OpenAPI documentation (there is no `#[get]` to read it from; `mount` takes it by hand).
    /// The unknown-route `404` stays purlin's JSON envelope; a fallback on your router is replaced.
    ///
    /// # Panics
    /// At build time, like axum itself, if a merged path and method collide with a
    /// controller route or another merged router.
    pub fn merge(mut self, router: Router) -> Self {
        self.merged.push(Box::new(move |_| Ok(router)));
        self
    }

    /// Like [`merge`](Self::merge), but the router is built *after* the module graph, from the
    /// container, so it can use providers: put a guard or interceptor on raw routes, or hand a
    /// service to a raw handler as axum state.
    ///
    /// ```ignore
    /// App::builder(AppModule)
    ///     .merge_with(|container| {
    ///         let guard = container.resolve::<BearerAuth>()?;
    ///         let admin = Router::new().route("/admin/reload", post(reload));
    ///         Ok(guarded_router(admin, guard))      // the same guard controllers use
    ///     })
    ///     .build()
    ///     .await?
    /// ```
    ///
    /// As with [`filter`](Self::filter), providers are resolved from the app's root, so they must be
    /// provided in the root module or a global module. A resolution error fails the build.
    pub fn merge_with(mut self, build: impl FnOnce(&Container) -> Result<Router, DiError> + Send + 'static) -> Self {
        self.merged.push(Box::new(build));
        self
    }

    /// Mounts a [`Mount`](crate::Mount): a raw router with guards, interceptors and OpenAPI docs added by builder
    /// calls, in the same order a controller applies them (guards, interceptors, handler). Like
    /// [`merge_with`](Self::merge_with) otherwise.
    pub fn mount(mut self, mount: crate::Mount) -> Self {
        let (build, mut docs) = mount.into_parts();
        self.merged.push(build);
        self.mounted_docs.append(&mut docs);
        self
    }

    /// Validates and builds the graph, then mounts every HTTP controller into one
    /// router. Unknown routes get the same JSON error envelope as every other failure.
    pub async fn build(self) -> Result<App, AppError> {
        let booted = bootstrap_with::<M>(self.overrides).await?;

        let mut router = Router::new();
        let mut docs = self.mounted_docs;
        for controller in booted.controllers {
            // Controllers for other transports are somebody else's business.
            if let Ok(mut routes) = controller.for_transport::<HttpTransport>() {
                router = router.merge(routes.router);
                docs.append(&mut routes.docs);
            }
        }
        for build in self.merged {
            router = router.merge(build(&booted.container)?);
        }
        let router = router.fallback(|| async { HttpError::not_found("route not found").into_response() });
        let router = with_request_scopes(router, Arc::clone(&booted.container), Arc::clone(&booted.scoped));

        let mut filters = Vec::with_capacity(self.filters.len());
        for resolve in self.filters {
            filters.push(resolve(&booted.container)?);
        }
        let router = with_filters(router, filters);
        let (stopping, stopped) = tokio::sync::watch::channel(false);
        let router = router.layer(axum::middleware::from_fn_with_state(
            stopped,
            crate::sse::end_on_shutdown,
        ));

        Ok(App {
            router,
            container: booted.container,
            modules: booted.modules,
            shutdown: booted.shutdown,
            docs,
            prefix: String::new(),
            stopping,
        })
    }
}

impl App {
    /// Validates and builds the graph rooted at `M`, then mounts every HTTP
    /// controller into one router. Shorthand for `App::builder(root).build()`.
    pub async fn create<M: Module>(root: M) -> Result<Self, AppError> {
        Self::builder(root).build().await
    }

    /// Starts configuring an app, e.g. to override providers in tests.
    pub fn builder<M: Module>(_root: M) -> AppBuilder<M> {
        AppBuilder {
            overrides: Overrides::new(),
            filters: Vec::new(),
            merged: Vec::new(),
            mounted_docs: Vec::new(),
            root: PhantomData,
        }
    }

    /// Applies a tower layer (tracing, CORS, timeouts, ..) to every route.
    pub fn layer<L>(mut self, layer: L) -> Self
    where
        L: Layer<Route> + Clone + Send + Sync + 'static,
        L::Service: Service<Request> + Clone + Send + Sync + 'static,
        <L::Service as Service<Request>>::Response: IntoResponse + 'static,
        <L::Service as Service<Request>>::Error: Into<Infallible> + 'static,
        <L::Service as Service<Request>>::Future: Send + 'static,
    {
        self.router = self.router.layer(layer);
        self
    }

    /// Runs `interceptor` around the whole app: every route, unknown paths and the
    /// error responses of guards and filters included. Because it wraps everything it
    /// runs *before* authentication, which is what a global rate limit or timeout wants.
    /// Later calls wrap earlier ones, so the last call is outermost.
    ///
    /// ```ignore
    /// app.interceptor(Timeout::secs(30)).interceptor(RateLimit::per_second(200))
    /// ```
    pub fn interceptor<I: crate::Interceptor>(mut self, interceptor: I) -> Self {
        self.router = self.router.layer(axum::middleware::from_fn_with_state(
            Arc::new(interceptor),
            |axum::extract::State(interceptor): axum::extract::State<Arc<I>>,
             request: Request,
             next: axum::middleware::Next| async move { interceptor.intercept(request, next).await },
        ));
        self
    }

    /// Nests every route under `prefix` (e.g. `"/api"`). An empty prefix or `"/"`
    /// is a no-op.
    pub fn global_prefix(mut self, prefix: &str) -> Self {
        let prefix = prefix.trim_end_matches('/');
        if !prefix.is_empty() {
            let fallback_free = std::mem::take(&mut self.router);
            self.router = Router::new()
                .nest(prefix, fallback_free)
                .fallback(|| async { HttpError::not_found("route not found").into_response() });
            self.prefix = prefix.to_owned();
        }
        self
    }

    /// The OpenAPI document for every HTTP route, paths including the global prefix.
    pub fn openapi(&self, info: &ApiInfo) -> utoipa::openapi::OpenApi {
        openapi::build(info, &self.prefix, &self.docs)
    }

    /// Serves the OpenAPI document at `<prefix>/openapi.json` and a Swagger UI page
    /// at `<prefix>/docs`. Call it after [`global_prefix`](Self::global_prefix) so
    /// the document lists the paths clients actually use.
    pub fn serve_openapi(mut self, info: ApiInfo) -> Self {
        let json = self.openapi(&info).to_json().unwrap_or_else(|error| {
            tracing::error!(%error, "could not serialise the OpenAPI document");
            "{}".to_owned()
        });
        let spec_url = format!("{}/openapi.json", self.prefix);
        let page = SWAGGER_PAGE
            .replace("{{title}}", &info.title)
            .replace("{{spec}}", &spec_url);
        self.router = self
            .router
            .route(
                &spec_url,
                get(move || {
                    let json = json.clone();
                    async move { ([(CONTENT_TYPE, "application/json")], json) }
                }),
            )
            .route(
                &format!("{}/docs", self.prefix),
                get(move || {
                    let page = page.clone();
                    async move { Html(page) }
                }),
            );
        self
    }

    /// The built providers, e.g. for health probes or tests.
    pub fn container(&self) -> &Container {
        &self.container
    }

    /// Module names in dependency order.
    pub fn modules(&self) -> &[&'static str] {
        &self.modules
    }

    /// A handle to the assembled router (cheap to clone), for tests that keep the
    /// app around to call [`shutdown`](Self::shutdown) afterwards.
    pub fn router(&self) -> Router {
        self.router.clone()
    }

    /// Runs every destroy hook, in reverse build order, and returns the errors.
    /// [`listen`](Self::listen) calls this itself; running it again is a no-op.
    pub async fn shutdown(&self) -> Vec<DiError> {
        self.stopping.send_replace(true);
        self.shutdown.run(&self.container).await
    }

    /// Consumes the app, yielding the router (handy with `tower::ServiceExt::oneshot`).
    pub fn into_router(self) -> Router {
        self.router
    }

    /// Binds `addr` and serves until Ctrl-C / SIGTERM, finishing in-flight requests.
    pub async fn listen(self, addr: impl ToSocketAddrs) -> Result<(), AppError> {
        let listener = TcpListener::bind(addr).await?;
        tracing::info!(address = %listener.local_addr()?, modules = ?self.modules, "purlin application listening");
        let stopping = self.stopping.clone();
        let served = axum::serve(
            listener,
            // Connection info lets interceptors (rate limiting) see the client address.
            self.router
                .clone()
                .into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            // Open event streams end with `close`, or the drain would never finish.
            stopping.send_replace(true);
        })
        .await;
        // Hooks run even if serving failed, so resources are always released.
        for error in self.shutdown().await {
            tracing::error!(%error, "shutdown hook failed");
        }
        Ok(served?)
    }
}

impl fmt::Debug for App {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("App")
            .field("modules", &self.modules)
            .field("container", &self.container)
            .finish_non_exhaustive()
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
}

/// A minimal Swagger UI page; the UI itself is loaded from a CDN.
const SWAGGER_PAGE: &str = r##"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>{{title}}</title>
  <link rel="stylesheet" href="https://unpkg.com/swagger-ui-dist@5.17.14/swagger-ui.css">
</head>
<body>
  <div id="ui"></div>
  <script src="https://unpkg.com/swagger-ui-dist@5.17.14/swagger-ui-bundle.js"></script>
  <script>window.ui = SwaggerUIBundle({ url: "{{spec}}", dom_id: "#ui" });</script>
</body>
</html>"##;
