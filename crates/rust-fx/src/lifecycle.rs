//! Start and stop hooks, and the handle providers use to register them.

use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};

use crate::error::BoxError;
use crate::value::BoxFuture;

pub(crate) type BoxHook = Box<dyn FnOnce() -> BoxFuture<'static, Result<(), BoxError>> + Send>;

/// What a hook may return: nothing, or a `Result` whose error is reported.
pub trait HookOutcome: Send + 'static {
    /// Converts the output into a uniform result.
    fn into_result(self) -> Result<(), BoxError>;
}

impl HookOutcome for () {
    fn into_result(self) -> Result<(), BoxError> {
        Ok(())
    }
}

impl<E: Into<BoxError> + Send + 'static> HookOutcome for Result<(), E> {
    fn into_result(self) -> Result<(), BoxError> {
        self.map_err(Into::into)
    }
}

fn boxed<F, Fut>(hook: F) -> BoxHook
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future + Send + 'static,
    Fut::Output: HookOutcome,
{
    Box::new(move || {
        let future = hook();
        Box::pin(async move { future.await.into_result() })
    })
}

/// A pair of optional start and stop callbacks, registered together.
///
/// Registered with [`Lifecycle::append`]. Start hooks run in registration order,
/// stop hooks in the reverse order, and a hook's stop callback only runs if its
/// start callback did (or there was none).
#[derive(Default)]
pub struct Hook {
    pub(crate) start: Option<BoxHook>,
    pub(crate) stop: Option<BoxHook>,
}

impl Hook {
    /// An empty hook.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the callback that runs when the app starts.
    #[must_use]
    pub fn on_start<F, Fut>(mut self, hook: F) -> Self
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future + Send + 'static,
        Fut::Output: HookOutcome,
    {
        self.start = Some(boxed(hook));
        self
    }

    /// Sets the callback that runs when the app stops.
    #[must_use]
    pub fn on_stop<F, Fut>(mut self, hook: F) -> Self
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future + Send + 'static,
        Fut::Output: HookOutcome,
    {
        self.stop = Some(boxed(hook));
        self
    }
}

pub(crate) struct Entry {
    pub(crate) owner: Arc<str>,
    pub(crate) start: Option<BoxHook>,
    pub(crate) stop: Option<BoxHook>,
}

/// Shared list of registered hooks, in registration order.
#[derive(Default)]
pub(crate) struct Registry {
    entries: Mutex<Vec<Entry>>,
}

impl Registry {
    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Entry>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn len(&self) -> usize {
        self.lock().len()
    }

    fn push(&self, entry: Entry) {
        self.lock().push(entry);
    }

    /// Takes the start callback of entry `index` (leaving the stop callback).
    pub(crate) fn take_start(&self, index: usize) -> Option<(Arc<str>, Option<BoxHook>)> {
        let mut entries = self.lock();
        let entry = entries.get_mut(index)?;
        Some((entry.owner.clone(), entry.start.take()))
    }

    /// Takes the stop callback of entry `index`.
    pub(crate) fn take_stop(&self, index: usize) -> Option<(Arc<str>, Option<BoxHook>)> {
        let mut entries = self.lock();
        let entry = entries.get_mut(index)?;
        Some((entry.owner.clone(), entry.stop.take()))
    }
}

/// The handle a provider takes to attach start and stop hooks to the app.
///
/// Take it as an argument (`fn database(life: Lifecycle) -> ..`). Hooks run once
/// every constructor and invoke has finished, so a hook may rely on the whole graph
/// being built.
#[derive(Clone)]
pub struct Lifecycle {
    registry: Arc<Registry>,
    owner: Arc<str>,
}

impl Lifecycle {
    pub(crate) fn new(registry: Arc<Registry>, owner: impl Into<Arc<str>>) -> Self {
        Self {
            registry,
            owner: owner.into(),
        }
    }

    /// Registers a hook.
    pub fn append(&self, hook: Hook) {
        self.registry.push(Entry {
            owner: self.owner.clone(),
            start: hook.start,
            stop: hook.stop,
        });
    }

    /// Registers a callback that runs when the app starts.
    pub fn on_start<F, Fut>(&self, hook: F)
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future + Send + 'static,
        Fut::Output: HookOutcome,
    {
        self.append(Hook::new().on_start(hook));
    }

    /// Registers a callback that runs when the app stops.
    pub fn on_stop<F, Fut>(&self, hook: F)
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future + Send + 'static,
        Fut::Output: HookOutcome,
    {
        self.append(Hook::new().on_stop(hook));
    }
}

impl std::fmt::Debug for Lifecycle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Lifecycle").field("owner", &self.owner).finish()
    }
}
