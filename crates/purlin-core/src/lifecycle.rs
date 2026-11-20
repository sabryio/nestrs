//! Lifecycle hooks: run code once the graph is built, and again at shutdown.
//!
//! Hooks are attached to a [`Provider`](crate::Provider) and run by
//! [`bootstrap`](crate::bootstrap) in dependency order: *init* hooks after every
//! provider has been built (dependencies first), *destroy* hooks in the reverse
//! order, so a service is always torn down before the things it depends on.

use std::future::Future;
use std::sync::Arc;

use crate::provider::BoxFuture;
use crate::{Container, DiError};

/// Implemented by providers that need to do async work once the whole graph exists
/// (warm a cache, run migrations, seed data).
///
/// Attach it with [`Provider::with_init`](crate::Provider::with_init).
pub trait OnModuleInit: Send + Sync + 'static {
    /// Runs once, after every provider has been built.
    fn on_module_init(&self) -> impl Future<Output = Result<(), DiError>> + Send;
}

/// Implemented by providers that must release resources at shutdown (flush
/// buffers, close pools).
///
/// Attach it with [`Provider::with_destroy`](crate::Provider::with_destroy).
pub trait OnModuleDestroy: Send + Sync + 'static {
    /// Runs once at shutdown, before the providers this one depends on are destroyed.
    fn on_module_destroy(&self) -> impl Future<Output = Result<(), DiError>> + Send;
}

/// A type-erased hook: resolves its target from the container and runs.
pub(crate) type Hook = Arc<dyn for<'a> Fn(&'a Container) -> BoxFuture<'a, Result<(), DiError>> + Send + Sync>;

/// The shutdown half of the lifecycle, produced by bootstrap.
///
/// Stops the running `fx` app: destroy hooks run once, in reverse build order. A failing hook
/// does not stop the others: every error is collected and returned. Calling
/// [`run`](Self::run) again is a no-op, so shutdown is safe to trigger from several places.
pub struct Shutdown {
    app: fx::tokio::sync::Mutex<Option<fx::App>>,
}

impl Shutdown {
    pub(crate) fn on_fx(app: fx::App) -> Self {
        Self {
            app: fx::tokio::sync::Mutex::new(Some(app)),
        }
    }

    /// Runs the destroy hooks (once) and returns every error.
    ///
    /// The container argument is not needed any more (each hook already holds its value); it
    /// stays so that existing callers keep compiling.
    pub async fn run(&self, _container: &Container) -> Vec<DiError> {
        let Some(mut app) = self.app.lock().await.take() else {
            return Vec::new();
        };
        match app.stop().await {
            Ok(()) => Vec::new(),
            Err(error) => crate::engine::stop_errors(error),
        }
    }
}

impl std::fmt::Debug for Shutdown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Shutdown")
    }
}
