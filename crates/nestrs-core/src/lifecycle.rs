//! Lifecycle hooks: run code once the graph is built, and again at shutdown.
//!
//! Hooks are attached to a [`Provider`](crate::Provider) and run by
//! [`bootstrap`](crate::bootstrap) in dependency order: *init* hooks after every
//! provider has been built (dependencies first), *destroy* hooks in the reverse
//! order, so a service is always torn down before the things it depends on.

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::provider::BoxFuture;
use crate::{Container, DiError, Token};

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
/// Runs each destroy hook once, in reverse build order. A failing hook does not
/// stop the others: every error is collected and returned. Calling [`run`](Self::run)
/// again is a no-op, so shutdown is safe to trigger from several places.
pub struct Shutdown {
    /// Already in execution order (last built first).
    hooks: Vec<(Token, Hook)>,
    done: AtomicBool,
}

impl Shutdown {
    pub(crate) fn new(hooks: Vec<(Token, Hook)>) -> Self {
        Self {
            hooks,
            done: AtomicBool::new(false),
        }
    }

    /// Runs the destroy hooks against `container`, returning every error.
    pub async fn run(&self, container: &Container) -> Vec<DiError> {
        if self.done.swap(true, Ordering::SeqCst) {
            return Vec::new();
        }
        let mut errors = Vec::new();
        for (token, hook) in &self.hooks {
            if let Err(error) = hook(container).await {
                errors.push(DiError::Hook {
                    provider: *token,
                    phase: "destroy",
                    message: error.to_string(),
                });
            }
        }
        errors
    }

    /// How many destroy hooks are registered.
    pub fn len(&self) -> usize {
        self.hooks.len()
    }

    /// Whether there is nothing to run at shutdown.
    pub fn is_empty(&self) -> bool {
        self.hooks.is_empty()
    }
}

impl std::fmt::Debug for Shutdown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shutdown").field("hooks", &self.hooks.len()).finish()
    }
}
