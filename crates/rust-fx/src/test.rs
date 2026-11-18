//! A headless harness for testing apps built on `fx` (`fxtest`).
//!
//! Enabled with the `test` feature:
//!
//! ```toml
//! [dev-dependencies]
//! rust-fx = { version = "0.1", features = ["test"] }
//! ```
//!
//! ```
//! use std::sync::Arc;
//!
//! trait Mailer: Send + Sync { fn send(&self, to: &str) -> String; }
//! struct Smtp;
//! impl Mailer for Smtp { fn send(&self, to: &str) -> String { format!("smtp:{to}") } }
//! struct Fake;
//! impl Mailer for Fake { fn send(&self, to: &str) -> String { format!("fake:{to}") } }
//!
//! # #[tokio::main(flavor = "current_thread")]
//! # async fn main() -> fx::Result<()> {
//! let mut app = fx::test::App::new()
//!     .provide(fx::bind!(dyn Mailer, || Arc::new(Smtp)))
//!     .replace::<dyn Mailer>(Arc::new(Fake));
//! app.start().await?;
//! assert_eq!(app.get::<dyn Mailer>().await?.send("a"), "fake:a");
//! app.stop().await?;
//! # Ok(()) }
//! ```

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use crate::descriptor::{IntoDecorator, IntoInvoke, IntoProvider};
use crate::error::Result;
use crate::func::Component;
use crate::module::Module;
use crate::observer::{Event, Observer};
use crate::options::Options;
use crate::shutdown::Shutdowner;

/// An observer that remembers every event, so a test can assert on what happened.
#[derive(Clone, Default)]
pub struct Recorder {
    events: Arc<Mutex<Vec<Event>>>,
}

impl Recorder {
    /// A fresh recorder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Every event so far, oldest first.
    pub fn events(&self) -> Vec<Event> {
        self.events.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Short descriptions of the events, convenient for `assert_eq!` on order.
    ///
    /// `provided Config`, `decorated Config`, `invoked <fn>`, `start <owner>`,
    /// `stop <owner>`, `started`, `stopped`.
    pub fn summary(&self) -> Vec<String> {
        self.events()
            .iter()
            .map(|event| match event {
                Event::Provided { key, .. } => format!("provided {key}"),
                Event::Decorated { key, .. } => format!("decorated {key}"),
                Event::Invoked { origin, .. } => format!("invoked {origin}"),
                Event::Hook { owner, phase, .. } => format!("{phase} {owner}"),
                Event::Started { .. } => "started".to_string(),
                Event::Stopped { .. } => "stopped".to_string(),
                Event::Signalled(signal) => format!("signalled {signal}"),
            })
            .collect()
    }
}

impl Observer for Recorder {
    fn on_event(&self, event: &Event) {
        self.events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(event.clone());
    }
}

/// An [`App`](crate::App) tuned for tests: 5 s timeouts, no signal handling, and a
/// [`Recorder`] attached.
///
/// Call [`stop`](Self::stop) when done so stop hooks run; there is no async `Drop`.
pub struct App {
    inner: crate::App,
    recorder: Recorder,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    /// An empty test app.
    pub fn new() -> Self {
        let recorder = Recorder::new();
        let inner = crate::App::new()
            .observer(recorder.clone())
            .start_timeout(Duration::from_secs(5))
            .stop_timeout(Duration::from_secs(5));
        Self { inner, recorder }
    }

    /// Adds a bundle of options.
    #[must_use]
    pub fn options(mut self, options: Options) -> Self {
        self.inner = self.inner.options(options);
        self
    }

    /// Registers a constructor.
    #[must_use]
    #[track_caller]
    pub fn provide<M>(mut self, provider: impl IntoProvider<M>) -> Self {
        self.inner = self.inner.provide(provider);
        self
    }

    /// Registers a startup function.
    #[must_use]
    #[track_caller]
    pub fn invoke<M>(mut self, invoke: impl IntoInvoke<M>) -> Self {
        self.inner = self.inner.invoke(invoke);
        self
    }

    /// Registers a decorator.
    #[must_use]
    #[track_caller]
    pub fn decorate<M>(mut self, decorator: impl IntoDecorator<M>) -> Self {
        self.inner = self.inner.decorate(decorator);
        self
    }

    /// Provides an existing value.
    #[must_use]
    #[track_caller]
    pub fn supply<T: Send + Sync + 'static>(mut self, value: T) -> Self {
        self.inner = self.inner.supply(value);
        self
    }

    /// Provides an existing shared value (possibly a trait object).
    #[must_use]
    #[track_caller]
    pub fn supply_arc<T: ?Sized + Send + Sync + 'static>(mut self, value: Arc<T>) -> Self {
        self.inner = self.inner.supply_arc(value);
        self
    }

    /// Imports a module.
    #[must_use]
    pub fn module<M: Module>(mut self) -> Self {
        self.inner = self.inner.module::<M>();
        self
    }

    /// Registers a component.
    #[must_use]
    pub fn component<C: Component>(mut self) -> Self {
        self.inner = self.inner.component::<C>();
        self
    }

    /// Replaces whatever provides `T` with `value`, dependencies and all.
    #[must_use]
    #[track_caller]
    pub fn replace<T: ?Sized + Send + Sync + 'static>(mut self, value: Arc<T>) -> Self {
        self.inner = self.inner.replace(value);
        self
    }

    /// The recorder attached to this app.
    pub fn recorder(&self) -> Recorder {
        self.recorder.clone()
    }

    /// A handle that requests shutdown.
    pub fn shutdowner(&self) -> Shutdowner {
        self.inner.shutdowner()
    }

    /// Checks the graph without running anything.
    ///
    /// # Errors
    ///
    /// The first wiring mistake found.
    pub fn validate(&self) -> Result<()> {
        self.inner.validate()
    }

    /// Starts the app.
    ///
    /// # Errors
    ///
    /// See [`crate::App::start`].
    pub async fn start(&mut self) -> Result<()> {
        self.inner.start().await
    }

    /// Stops the app.
    ///
    /// # Errors
    ///
    /// See [`crate::App::stop`].
    pub async fn stop(&mut self) -> Result<()> {
        self.inner.stop().await
    }

    /// Fetches a value from the running app.
    ///
    /// # Errors
    ///
    /// See [`crate::App::get`].
    pub async fn get<T: ?Sized + Send + Sync + 'static>(&mut self) -> Result<Arc<T>> {
        self.inner.get::<T>().await
    }

    /// Fetches a named value from the running app.
    ///
    /// # Errors
    ///
    /// See [`crate::App::get_named`].
    pub async fn get_named<T: ?Sized + Send + Sync + 'static>(&mut self, name: &'static str) -> Result<Arc<T>> {
        self.inner.get_named::<T>(name).await
    }
}
