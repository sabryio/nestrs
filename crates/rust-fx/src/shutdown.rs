//! Asking the app to stop, from inside it or from a signal.

use std::fmt;
use std::sync::Arc;

use tokio::sync::watch;

/// Why [`App::done`](crate::App::done) returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    /// SIGINT / Ctrl-C.
    Interrupt,
    /// SIGTERM.
    Terminate,
    /// Somebody called [`Shutdowner::shutdown`] or [`Shutdowner::shutdown_with`].
    Requested(i32),
}

impl Signal {
    /// The process exit code this signal maps to (`0` unless requested otherwise).
    pub fn exit_code(self) -> i32 {
        match self {
            Self::Requested(code) => code,
            Self::Interrupt | Self::Terminate => 0,
        }
    }
}

impl fmt::Display for Signal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Interrupt => f.write_str("interrupt"),
            Self::Terminate => f.write_str("terminate"),
            Self::Requested(code) => write!(f, "shutdown requested (exit code {code})"),
        }
    }
}

/// Lets any provider, invoke or hook ask the app to shut down gracefully.
///
/// Take it as an argument. The first request wins; later calls are ignored.
#[derive(Clone)]
pub struct Shutdowner {
    tx: Arc<watch::Sender<Option<i32>>>,
}

impl Shutdowner {
    pub(crate) fn new() -> Self {
        Self {
            tx: Arc::new(watch::channel(None).0),
        }
    }

    /// Requests a shutdown with exit code `0`.
    pub fn shutdown(&self) {
        self.shutdown_with(0);
    }

    /// Requests a shutdown with the given exit code.
    pub fn shutdown_with(&self, code: i32) {
        self.tx.send_if_modified(|slot| {
            let first = slot.is_none();
            if first {
                *slot = Some(code);
            }
            first
        });
    }

    /// Whether a shutdown has been requested.
    pub fn is_requested(&self) -> bool {
        self.tx.borrow().is_some()
    }

    pub(crate) async fn requested(&self) -> i32 {
        let mut rx = self.tx.subscribe();
        match rx.wait_for(Option::is_some).await {
            Ok(slot) => slot.unwrap_or(0),
            // The sender lives in `self`, so this cannot happen; never resolve.
            Err(_) => std::future::pending().await,
        }
    }
}

impl fmt::Debug for Shutdowner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Shutdowner")
            .field("requested", &self.tx.borrow().is_some())
            .finish()
    }
}

/// Resolves when the process receives SIGINT or SIGTERM.
pub(crate) async fn os_signal() -> Signal {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let term = signal(SignalKind::terminate());
        let int = signal(SignalKind::interrupt());
        match (term, int) {
            (Ok(mut term), Ok(mut int)) => tokio::select! {
                _ = term.recv() => Signal::Terminate,
                _ = int.recv() => Signal::Interrupt,
            },
            _ => {
                tracing::warn!("could not install signal handlers; only shutdown requests will stop the app");
                std::future::pending().await
            }
        }
    }
    #[cfg(not(unix))]
    {
        match tokio::signal::ctrl_c().await {
            Ok(()) => Signal::Interrupt,
            Err(_) => std::future::pending().await,
        }
    }
}
