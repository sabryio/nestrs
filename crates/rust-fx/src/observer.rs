//! Observing what the app does, the way `fx.WithLogger` does.

use std::time::Duration;

use crate::error::Phase;
use crate::shutdown::Signal;

/// Something that happened while the app ran.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Event {
    /// A constructor produced a value.
    Provided {
        /// The constructor.
        origin: String,
        /// The value it produced.
        key: String,
        /// How long it took.
        elapsed: Duration,
    },
    /// A decorator wrapped a value.
    Decorated {
        /// The decorator.
        origin: String,
        /// The value it wrapped.
        key: String,
        /// How long it took.
        elapsed: Duration,
    },
    /// An invoked function finished.
    Invoked {
        /// The function.
        origin: String,
        /// How long it took.
        elapsed: Duration,
    },
    /// A hook finished.
    Hook {
        /// The provider that registered it.
        owner: String,
        /// Start or stop.
        phase: Phase,
        /// How long it took.
        elapsed: Duration,
        /// Its error, if it failed.
        error: Option<String>,
    },
    /// The app finished starting.
    Started {
        /// How long starting took in total.
        elapsed: Duration,
    },
    /// The app finished stopping.
    Stopped {
        /// How long stopping took in total.
        elapsed: Duration,
    },
    /// A signal or shutdown request arrived.
    Signalled(Signal),
}

/// Receives every [`Event`]. Implementations must be cheap and must not block.
pub trait Observer: Send + Sync + 'static {
    /// Called for each event, in the order they happen.
    fn on_event(&self, event: &Event);
}

/// The default observer: emits `tracing` events (debug for progress, warn for failures).
#[derive(Debug, Default, Clone, Copy)]
pub struct Tracing;

impl Observer for Tracing {
    fn on_event(&self, event: &Event) {
        match event {
            Event::Provided { origin, key, elapsed } => {
                tracing::debug!(target: "fx", %origin, %key, ?elapsed, "provided");
            }
            Event::Decorated { origin, key, elapsed } => {
                tracing::debug!(target: "fx", %origin, %key, ?elapsed, "decorated");
            }
            Event::Invoked { origin, elapsed } => {
                tracing::debug!(target: "fx", %origin, ?elapsed, "invoked");
            }
            Event::Hook {
                owner,
                phase,
                elapsed,
                error: None,
            } => {
                tracing::debug!(target: "fx", %owner, %phase, ?elapsed, "hook ran");
            }
            Event::Hook {
                owner,
                phase,
                elapsed,
                error: Some(error),
            } => {
                tracing::warn!(target: "fx", %owner, %phase, ?elapsed, %error, "hook failed");
            }
            Event::Started { elapsed } => tracing::info!(target: "fx", ?elapsed, "started"),
            Event::Stopped { elapsed } => tracing::info!(target: "fx", ?elapsed, "stopped"),
            Event::Signalled(signal) => tracing::info!(target: "fx", %signal, "received signal"),
        }
    }
}

/// An observer that ignores everything.
#[derive(Debug, Default, Clone, Copy)]
pub struct Silent;

impl Observer for Silent {
    fn on_event(&self, _event: &Event) {}
}
