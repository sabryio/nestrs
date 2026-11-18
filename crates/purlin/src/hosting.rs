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
        .provide(fx::provide(boot::<M>).on_stop(teardown))
        .invoke(force_start)
}

/// Forces construction at start, even when nothing else depends on [`Hosted`].
fn force_start(_hosted: Arc<Hosted>) {}
