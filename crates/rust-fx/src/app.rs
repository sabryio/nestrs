//! [`App`]: build the graph, start it, wait, stop it.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::descriptor::{IntoDecorator, IntoInvoke, IntoProvider};
use crate::error::{Error, Phase, Result};
use crate::extract::{Args, Context, Resolved};
use crate::func::Component;
use crate::graph::{Binding, NodeId, NodeKind, Plan};
use crate::key::Key;
use crate::lifecycle::{Lifecycle, Registry};
use crate::module::Module;
use crate::observer::{Event, Observer, Tracing};
use crate::options::Options;
use crate::scope::Scopes;
use crate::shutdown::{Shutdowner, Signal, os_signal};
use crate::value::{Stored, load};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);

/// An application: a set of [`Options`], a runtime and a lifecycle.
///
/// ```
/// use std::sync::Arc;
///
/// struct Greeter;
/// fn greeter() -> Arc<Greeter> { Arc::new(Greeter) }
/// fn hello(_: Arc<Greeter>) { println!("hello"); }
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> fx::Result<()> {
/// fx::App::new().constructor(greeter).invoke(hello).run_until_started().await?;
/// # Ok(()) }
/// ```
pub struct App {
    options: Options,
    observer: Arc<dyn Observer>,
    start_timeout: Duration,
    stop_timeout: Duration,
    shutdowner: Shutdowner,
    eager: bool,
    runtime: Option<Runtime>,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    /// An empty app with a 15 s start and stop timeout and the [`Tracing`] observer.
    pub fn new() -> Self {
        Self {
            options: Options::new(),
            observer: Arc::new(Tracing),
            start_timeout: DEFAULT_TIMEOUT,
            stop_timeout: DEFAULT_TIMEOUT,
            shutdowner: Shutdowner::new(),
            eager: false,
            runtime: None,
        }
    }

    /// Builds *every* singleton at start, not only what the invokes reach.
    ///
    /// By default construction is lazy: a provider nobody needs is checked but never run.
    /// Eager start suits frameworks that treat registration as intent (a provider's start
    /// hook should run even if nothing injects it). Hooks still run in dependency order.
    #[must_use]
    pub fn eager(mut self) -> Self {
        self.eager = true;
        self
    }

    /// Adds a bundle of options.
    #[must_use]
    pub fn options(mut self, options: Options) -> Self {
        self.options = self.options.merge(options);
        self
    }

    /// Registers a constructor. See [`Options::constructor`].
    #[must_use]
    #[track_caller]
    pub fn constructor<M>(mut self, provider: impl IntoProvider<M>) -> Self {
        self.options = self.options.constructor(provider);
        self
    }

    /// Registers a startup function. See [`Options::invoke`].
    #[must_use]
    #[track_caller]
    pub fn invoke<M>(mut self, invoke: impl IntoInvoke<M>) -> Self {
        self.options = self.options.invoke(invoke);
        self
    }

    /// Registers a decorator. See [`Options::decorate`].
    #[must_use]
    #[track_caller]
    pub fn decorate<M>(mut self, decorator: impl IntoDecorator<M>) -> Self {
        self.options = self.options.decorate(decorator);
        self
    }

    /// Provides an existing value. See [`Options::supply`].
    #[must_use]
    #[track_caller]
    pub fn supply<T: Send + Sync + 'static>(mut self, value: T) -> Self {
        self.options = self.options.supply(value);
        self
    }

    /// Provides an existing shared value. See [`Options::supply_arc`].
    #[must_use]
    #[track_caller]
    pub fn supply_arc<T: ?Sized + Send + Sync + 'static>(mut self, value: Arc<T>) -> Self {
        self.options = self.options.supply_arc(value);
        self
    }

    /// Imports a module.
    #[must_use]
    pub fn module<M: Module>(mut self) -> Self {
        self.options = self.options.module::<M>();
        self
    }

    /// Registers a component. See [`Options::component`].
    #[must_use]
    pub fn component<C: Component>(mut self) -> Self {
        self.options = self.options.component::<C>();
        self
    }

    /// Replaces a provided value. See [`Options::replace`].
    #[must_use]
    #[track_caller]
    pub fn replace<T: ?Sized + Send + Sync + 'static>(mut self, value: Arc<T>) -> Self {
        self.options = self.options.replace(value);
        self
    }

    /// Sets how long [`start`](Self::start) may take (default 15 s).
    #[must_use]
    pub fn start_timeout(mut self, limit: Duration) -> Self {
        self.start_timeout = limit;
        self
    }

    /// Sets how long [`stop`](Self::stop) may take (default 15 s).
    #[must_use]
    pub fn stop_timeout(mut self, limit: Duration) -> Self {
        self.stop_timeout = limit;
        self
    }

    /// Replaces the observer that receives [`Event`]s.
    #[must_use]
    pub fn observer(mut self, observer: impl Observer) -> Self {
        self.observer = Arc::new(observer);
        self
    }

    /// A handle that asks the app to stop, usable before the app starts.
    pub fn shutdowner(&self) -> Shutdowner {
        self.shutdowner.clone()
    }

    /// Checks the whole graph without constructing anything: missing, invisible,
    /// duplicate and circular dependencies, bad exports and unknown replacements
    /// (`fx.ValidateApp`).
    ///
    /// # Errors
    ///
    /// The first wiring mistake found.
    pub fn validate(&self) -> Result<()> {
        Plan::build(self.options.clone()).map(drop)
    }

    /// Whether [`start`](Self::start) has succeeded and [`stop`](Self::stop) has not run.
    pub fn is_running(&self) -> bool {
        self.runtime.is_some()
    }

    /// Validates the graph, runs the invokes (building what they need, in dependency
    /// order), then runs the start hooks.
    ///
    /// If anything fails, the hooks that already started are stopped again, in
    /// reverse order, before the error is returned. The whole call is bounded by the
    /// start timeout.
    ///
    /// # Errors
    ///
    /// A wiring mistake, a failing constructor, invoke or hook, or a timeout.
    pub async fn start(&mut self) -> Result<()> {
        if self.runtime.is_some() {
            return Err(Error::AlreadyStarted);
        }
        let began = Instant::now();
        let plan = Plan::build(self.options.clone())?;
        let registry = Arc::new(Registry::default());
        self.runtime = Some(Runtime::new(
            plan,
            registry,
            self.shutdowner.clone(),
            self.observer.clone(),
            self.eager,
        ));

        let limit = self.start_timeout;
        let outcome = {
            let runtime = self.runtime.as_mut().ok_or(Error::NotStarted)?;
            match tokio::time::timeout(limit, runtime.start()).await {
                Ok(result) => result,
                Err(_) => Err(Error::Timeout {
                    phase: Phase::Start,
                    limit,
                }),
            }
        };

        if let Err(error) = outcome {
            if let Some(mut runtime) = self.runtime.take() {
                // Best effort: undo whatever did start; the start error is the one to report.
                let _ = runtime.stop().await;
            }
            return Err(error);
        }
        self.observer.on_event(&Event::Started {
            elapsed: began.elapsed(),
        });
        Ok(())
    }

    /// Opens request scopes over the running app; see [`Scopes`].
    ///
    /// # Errors
    ///
    /// [`Error::NotStarted`] before [`start`](Self::start).
    pub fn scopes(&self) -> Result<Scopes> {
        let runtime = self.runtime.as_ref().ok_or(Error::NotStarted)?;
        Ok(Scopes::new(
            runtime.plan.clone(),
            runtime.values.clone(),
            self.shutdowner.clone(),
        ))
    }

    /// Runs the stop hooks in reverse order, bounded by the stop timeout. Every hook
    /// runs even if an earlier one failed.
    ///
    /// # Errors
    ///
    /// [`Error::Stop`] with every failure, or [`Error::Timeout`].
    pub async fn stop(&mut self) -> Result<()> {
        let mut runtime = self.runtime.take().ok_or(Error::NotStarted)?;
        let began = Instant::now();
        let limit = self.stop_timeout;
        let errors = match tokio::time::timeout(limit, runtime.stop()).await {
            Ok(errors) => errors,
            Err(_) => vec![Error::Timeout {
                phase: Phase::Stop,
                limit,
            }],
        };
        self.observer.on_event(&Event::Stopped {
            elapsed: began.elapsed(),
        });
        match errors.len() {
            0 => Ok(()),
            1 if matches!(errors[0], Error::Timeout { .. }) => {
                Err(errors.into_iter().next().unwrap_or(Error::NotStarted))
            }
            _ => Err(Error::Stop { errors }),
        }
    }

    /// Waits for SIGINT, SIGTERM or a [`Shutdowner`] request (`app.Done()`).
    pub async fn done(&self) -> Signal {
        let signal = tokio::select! {
            signal = os_signal() => signal,
            code = self.shutdowner.requested() => Signal::Requested(code),
        };
        self.observer.on_event(&Event::Signalled(signal));
        signal
    }

    /// `start`, wait for [`done`](Self::done), `stop`; returns the exit code (`0`
    /// unless a [`Shutdowner::shutdown_with`] said otherwise).
    ///
    /// # Errors
    ///
    /// Whatever `start` or `stop` fails with.
    pub async fn run(mut self) -> Result<i32> {
        self.start().await?;
        let signal = self.done().await;
        self.stop().await?;
        Ok(signal.exit_code())
    }

    /// `start` then `stop`, without waiting: handy for one-shot programs whose work
    /// happens in invokes, and for doctests.
    ///
    /// # Errors
    ///
    /// Whatever `start` or `stop` fails with.
    pub async fn run_until_started(mut self) -> Result<()> {
        self.start().await?;
        self.stop().await
    }

    /// Fetches a value from a running app, building it first if nothing needed it yet.
    ///
    /// Only values visible from the app root can be fetched: provided at the root, or
    /// exported by an imported module.
    ///
    /// # Errors
    ///
    /// [`Error::NotStarted`], [`Error::Missing`], or whatever building it fails with.
    pub async fn get<T: ?Sized + Send + Sync + 'static>(&mut self) -> Result<Arc<T>> {
        self.get_by_key(Key::of::<T>()).await
    }

    /// Like [`get`](Self::get), for a value registered under `name`.
    ///
    /// # Errors
    ///
    /// See [`get`](Self::get).
    pub async fn get_named<T: ?Sized + Send + Sync + 'static>(&mut self, name: &'static str) -> Result<Arc<T>> {
        self.get_by_key(Key::named::<T>(name)).await
    }

    async fn get_by_key<T: ?Sized + Send + Sync + 'static>(&mut self, key: Key) -> Result<Arc<T>> {
        let limit = self.start_timeout;
        let runtime = self.runtime.as_mut().ok_or(Error::NotStarted)?;
        match tokio::time::timeout(limit, runtime.fetch(key)).await {
            Ok(stored) => {
                let stored = stored?;
                load::<T>(&stored).ok_or_else(|| Error::internal(format!("`{key}` has an unexpected type")))
            }
            Err(_) => Err(Error::Timeout {
                phase: Phase::Start,
                limit,
            }),
        }
    }
}

impl std::fmt::Debug for App {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("App")
            .field("options", &self.options)
            .field("running", &self.runtime.is_some())
            .finish()
    }
}

/// The built graph plus everything constructed so far.
struct Runtime {
    plan: Arc<Plan>,
    values: Vec<Option<Stored>>,
    done: Vec<bool>,
    registry: Arc<Registry>,
    /// Hook entries whose start phase has completed.
    started: usize,
    shutdowner: Shutdowner,
    observer: Arc<dyn Observer>,
    eager: bool,
}

impl Runtime {
    fn new(
        plan: Plan,
        registry: Arc<Registry>,
        shutdowner: Shutdowner,
        observer: Arc<dyn Observer>,
        eager: bool,
    ) -> Self {
        let count = plan.nodes.len();
        Self {
            plan: Arc::new(plan),
            values: vec![None; count],
            done: vec![false; count],
            registry,
            started: 0,
            shutdowner,
            observer,
            eager,
        }
    }

    async fn start(&mut self) -> Result<()> {
        for node in self.plan.invoke_nodes.clone() {
            self.ensure(node).await?;
        }
        if self.eager {
            for node in self.plan.singleton_providers() {
                self.ensure(node).await?;
            }
        }
        // Scoped values are built later, per request, from singletons that must exist by then.
        for node in self.plan.singletons_behind_scoped() {
            self.ensure(node).await?;
        }
        self.run_pending_starts().await
    }

    /// Builds `node` and everything it needs that is not built yet.
    async fn ensure(&mut self, node: NodeId) -> Result<()> {
        for next in self.plan.build_order(node, &self.done) {
            self.run_node(next).await?;
        }
        Ok(())
    }

    async fn fetch(&mut self, key: Key) -> Result<Stored> {
        let node = *self.plan.root_view.get(&key).ok_or_else(|| Error::Missing {
            key,
            requester: "App::get".into(),
            path: Box::default(),
        })?;
        self.ensure(node).await?;
        // Anything built just now may have registered hooks; the app is already
        // running, so they start immediately.
        self.run_pending_starts().await?;
        self.values[node]
            .clone()
            .ok_or_else(|| Error::internal(format!("`{key}` was not built")))
    }

    fn resolve_args(&self, node: NodeId) -> Result<Vec<Resolved>> {
        let value = |dep: NodeId| {
            self.values[dep]
                .clone()
                .ok_or_else(|| Error::internal("a dependency was not built before its consumer"))
        };
        self.plan.nodes[node]
            .args
            .iter()
            .map(|binding| match binding {
                Binding::One(dep) => value(*dep).map(Resolved::One),
                Binding::Maybe(None) => Ok(Resolved::Maybe(None)),
                Binding::Maybe(Some(dep)) => value(*dep).map(|v| Resolved::Maybe(Some(v))),
                Binding::Many(deps) => deps
                    .iter()
                    .map(|d| value(*d))
                    .collect::<Result<Vec<_>>>()
                    .map(Resolved::Many),
            })
            .collect()
    }

    async fn run_node(&mut self, node: NodeId) -> Result<()> {
        let began = Instant::now();
        let mut args = Args::new(self.resolve_args(node)?);
        let origin = self.plan.origin(node);
        let key = self.plan.nodes[node].key;
        let label = key.map_or_else(|| origin.to_string(), |k| format!("{k} by {origin}"));
        let context = Context {
            lifecycle: Lifecycle::new(self.registry.clone(), label),
            shutdowner: self.shutdowner.clone(),
        };

        match self.plan.nodes[node].kind {
            NodeKind::Provider(index) => {
                let build = self.plan.providers[index].provider.build.clone();
                let stored = build(&mut args, &context)?.await.map_err(|source| Error::Build {
                    key: key.map(|k| k.to_string()).unwrap_or_default().into(),
                    origin: origin.to_string().into(),
                    source,
                })?;
                self.values[node] = Some(stored);
                self.observer.on_event(&Event::Provided {
                    origin: origin.to_string(),
                    key: key.map(|k| k.to_string()).unwrap_or_default(),
                    elapsed: began.elapsed(),
                });
            }
            NodeKind::Decorator { entry, .. } => {
                let build = self.plan.decorators[entry].decorator.build.clone();
                let stored = build(&mut args, &context)?.await.map_err(|source| Error::Build {
                    key: key.map(|k| k.to_string()).unwrap_or_default().into(),
                    origin: origin.to_string().into(),
                    source,
                })?;
                self.values[node] = Some(stored);
                self.observer.on_event(&Event::Decorated {
                    origin: origin.to_string(),
                    key: key.map(|k| k.to_string()).unwrap_or_default(),
                    elapsed: began.elapsed(),
                });
            }
            NodeKind::Invoke(index) => {
                let build = self.plan.invokes[index].invoke.build.clone();
                build(&mut args, &context)?.await.map_err(|source| Error::Invoke {
                    origin: origin.to_string().into(),
                    source,
                })?;
                self.observer.on_event(&Event::Invoked {
                    origin: origin.to_string(),
                    elapsed: began.elapsed(),
                });
            }
        }
        self.done[node] = true;
        Ok(())
    }

    /// Runs the start callback of every hook registered since the last call, in order.
    async fn run_pending_starts(&mut self) -> Result<()> {
        while self.started < self.registry.len() {
            let index = self.started;
            if let Some((owner, Some(hook))) = self.registry.take_start(index) {
                let began = Instant::now();
                let result = hook().await;
                self.observer.on_event(&Event::Hook {
                    owner: owner.to_string(),
                    phase: Phase::Start,
                    elapsed: began.elapsed(),
                    error: result.as_ref().err().map(ToString::to_string),
                });
                if let Err(source) = result {
                    return Err(Error::Hook {
                        owner: owner.to_string().into(),
                        phase: Phase::Start,
                        source,
                    });
                }
            }
            // Counted as started once its start callback ran (or it had none).
            self.started += 1;
        }
        Ok(())
    }

    /// Stops every started hook in reverse order and returns all failures.
    async fn stop(&mut self) -> Vec<Error> {
        let mut errors = Vec::new();
        for index in (0..self.started).rev() {
            let Some((owner, Some(hook))) = self.registry.take_stop(index) else {
                continue;
            };
            let began = Instant::now();
            let result = hook().await;
            self.observer.on_event(&Event::Hook {
                owner: owner.to_string(),
                phase: Phase::Stop,
                elapsed: began.elapsed(),
                error: result.as_ref().err().map(ToString::to_string),
            });
            if let Err(source) = result {
                errors.push(Error::Hook {
                    owner: owner.to_string().into(),
                    phase: Phase::Stop,
                    source,
                });
            }
        }
        self.started = 0;
        errors
    }
}
