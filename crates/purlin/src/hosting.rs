//! Hosting a purlin module graph inside an [`fx::App`].
//!
//! purlin's module system (`#[module]`, `#[injectable]`, request scopes) still runs on
//! `purlin-core`. This bridge lets such a graph live *inside* an fx application, so the two
//! layers share one lifecycle, one set of signals and one shutdown path:
//!
//! ```no_run
//! # use purlin::{fx, hosting};
//! # #[purlin::module] struct AppModule;
//! # async fn demo() -> fx::Result<()> {
//! fx::App::new().options(hosting::hosted::<AppModule>()).run().await?;
//! # Ok(()) }
//! ```
//!
//! Design: [`Hosted`] is an ordinary fx component. Its constructor bootstraps the graph (so
//! wiring mistakes fail the fx start phase), its stop hook runs purlin's destroy hooks in
//! reverse order, and an invoke forces construction even when nothing else depends on it.
//! Other fx components may take `Arc<Hosted>` to reach purlin singletons.

use std::sync::Arc;

use fx::Options;
use std::sync::Mutex;

use purlin_core::{Container, DiError, Module, MountedController, ScopedProviders, Shutdown};

/// A bootstrapped purlin module graph, owned by an fx application.
pub struct Hosted {
    container: Arc<Container>,
    scoped: Arc<ScopedProviders>,
    modules: Vec<&'static str>,
    shutdown: Shutdown,
    // Mounted controllers are `Send` but not `Sync`; the mutex makes the whole host shareable.
    controllers: Mutex<Vec<MountedController>>,
}

impl fx::Component for Hosted {}

impl Hosted {
    /// Resolves a built singleton (a concrete type or `dyn Trait`).
    ///
    /// # Errors
    /// [`DiError::NotFound`] when no provider of that type exists in the graph.
    pub fn resolve<T: ?Sized + Send + Sync + 'static>(&self) -> Result<Arc<T>, DiError> {
        self.container.resolve::<T>()
    }

    /// Module names in dependency order (imports first).
    #[must_use]
    pub fn modules(&self) -> &[&'static str] {
        &self.modules
    }

    /// Every built singleton.
    #[must_use]
    pub fn container(&self) -> Arc<Container> {
        Arc::clone(&self.container)
    }

    /// Recipes for request-scoped and transient providers.
    #[must_use]
    pub fn scoped(&self) -> Arc<ScopedProviders> {
        Arc::clone(&self.scoped)
    }

    /// Hands the mounted controllers to a transport adapter (an HTTP server, say).
    ///
    /// They can be taken once; later calls return an empty list.
    pub fn take_controllers(&self) -> Vec<MountedController> {
        std::mem::take(
            &mut *self
                .controllers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }
}

impl std::fmt::Debug for Hosted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hosted").field("modules", &self.modules).finish()
    }
}

/// Raised when purlin's destroy hooks fail while the fx app stops.
#[derive(Debug, thiserror::Error)]
#[error("{count} purlin destroy hook(s) failed: {summary}")]
pub struct ShutdownFailed {
    count: usize,
    summary: String,
}

async fn boot<M: Module>() -> Result<Hosted, DiError> {
    let booted = purlin_core::bootstrap::<M>().await?;
    Ok(Hosted {
        container: booted.container,
        scoped: booted.scoped,
        modules: booted.modules,
        shutdown: booted.shutdown,
        controllers: Mutex::new(booted.controllers),
    })
}

async fn teardown(hosted: Arc<Hosted>) -> Result<(), ShutdownFailed> {
    let errors = hosted.shutdown.run(&hosted.container).await;
    if errors.is_empty() {
        return Ok(());
    }
    Err(ShutdownFailed {
        count: errors.len(),
        summary: errors.iter().map(ToString::to_string).collect::<Vec<_>>().join("; "),
    })
}

/// fx options that host the purlin module graph rooted at `M`.
///
/// Registers [`Hosted`] (bootstrapped on construction, torn down on stop) and an invoke that
/// makes sure it is built at start.
#[must_use]
pub fn hosted<M: Module>() -> Options {
    Options::new()
        .constructor(fx::constructor(boot::<M>).on_stop(teardown))
        .invoke(force_start)
}

/// Forces construction at start, even when nothing else depends on [`Hosted`].
fn force_start(_hosted: Arc<Hosted>) {}

#[cfg(feature = "http")]
pub use serve::{HttpServer, serve, serve_with};

/// The HTTP transport as an fx component.
#[cfg(feature = "http")]
mod serve {
    use std::net::SocketAddr;
    use std::sync::{Arc, Mutex, PoisonError};

    use fx::Options;
    use purlin_http::{App, AppError};
    use tokio::net::TcpListener;
    use tokio::sync::oneshot;
    use tokio::task::JoinHandle;

    use super::Hosted;

    type Configure = Arc<dyn Fn(App) -> App + Send + Sync>;

    /// A bound HTTP listener, serving a hosted graph's controllers.
    ///
    /// Construction builds the router and binds the socket (so a bad address fails fx's start
    /// phase); the start hook begins serving; the stop hook drains in-flight requests, ends open
    /// event streams and waits for the server task.
    pub struct HttpServer {
        local_addr: SocketAddr,
        pending: Mutex<Option<(App, TcpListener)>>,
        stop: Mutex<Option<oneshot::Sender<()>>>,
        task: Mutex<Option<JoinHandle<Result<(), AppError>>>>,
    }

    impl fx::Component for HttpServer {}

    impl HttpServer {
        /// The address actually bound (useful with port `0`).
        #[must_use]
        pub fn local_addr(&self) -> SocketAddr {
            self.local_addr
        }

        fn spawn(&self) {
            let Some((app, listener)) = lock(&self.pending).take() else {
                return;
            };
            let (stop, stopped) = oneshot::channel();
            *lock(&self.stop) = Some(stop);
            tracing::info!(address = %self.local_addr, "purlin http server listening");
            let task = tokio::spawn(app.serve_on(listener, async move {
                let _ = stopped.await;
            }));
            *lock(&self.task) = Some(task);
        }

        async fn halt(&self) -> Result<(), AppError> {
            if let Some(stop) = lock(&self.stop).take() {
                let _ = stop.send(());
            }
            let task = lock(&self.task).take();
            match task {
                Some(task) => task.await.map_err(|error| AppError::Io(std::io::Error::other(error)))?,
                None => Ok(()),
            }
        }
    }

    fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        mutex.lock().unwrap_or_else(PoisonError::into_inner)
    }

    async fn build(host: Arc<Hosted>, address: Arc<str>, configure: Configure) -> Result<HttpServer, AppError> {
        let app = App::from_bootstrapped(
            host.container(),
            host.scoped(),
            host.take_controllers(),
            host.modules().to_vec(),
        )?;
        let app = configure(app);
        let listener = TcpListener::bind(&*address).await?;
        Ok(HttpServer {
            local_addr: listener.local_addr()?,
            pending: Mutex::new(Some((app, listener))),
            stop: Mutex::new(None),
            task: Mutex::new(None),
        })
    }

    /// fx options that serve the hosted graph's HTTP controllers on `address`.
    ///
    /// Combine with [`hosted`](super::hosted):
    ///
    /// ```no_run
    /// # use purlin::{fx, hosting};
    /// # #[purlin::module] struct AppModule;
    /// # async fn demo() -> fx::Result<()> {
    /// fx::App::new()
    ///     .options(hosting::hosted::<AppModule>())
    ///     .options(hosting::serve("0.0.0.0:3000"))
    ///     .run()
    ///     .await?;
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn serve(address: impl Into<String>) -> Options {
        serve_with(address, |app| app)
    }

    /// Like [`serve`], with a hook to customise the [`App`] (layers, prefix, OpenAPI) first.
    #[must_use]
    pub fn serve_with(address: impl Into<String>, configure: impl Fn(App) -> App + Send + Sync + 'static) -> Options {
        let address: Arc<str> = address.into().into();
        let configure: Configure = Arc::new(configure);
        Options::new()
            .constructor(
                fx::constructor(move |host: Arc<Hosted>| build(host, Arc::clone(&address), Arc::clone(&configure)))
                    .on_start(|server: Arc<HttpServer>| async move { server.spawn() })
                    .on_stop(|server: Arc<HttpServer>| async move { server.halt().await }),
            )
            .invoke(|_server: Arc<HttpServer>| {})
    }
}
