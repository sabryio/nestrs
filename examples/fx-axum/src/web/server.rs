//! The HTTP server as an fx component.
//!
//! Binding happens in the (async, fallible) constructor, so a busy port fails the start
//! with a clear error before anything else runs. The accept loop starts on the start hook
//! and stops gracefully on the stop hook; fx runs stop hooks in reverse start order.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError};

use axum::Router;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use super::ControllerPort;

/// Where to listen. Port `0` asks the OS for a free one (used by tests).
#[derive(Debug, Clone, Copy)]
pub struct ServerConfig {
    /// The address to bind.
    pub addr: SocketAddr,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            addr: SocketAddr::from(([127, 0, 0, 1], 3000)),
        }
    }
}

/// A bound server. Inject it to learn the actual address.
pub struct Server {
    addr: SocketAddr,
}

impl Server {
    /// The address the server is listening on.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }
}

impl fx::Component for Server {}

/// Merges every controller's routes, binds the listener, and ties serving to the app's
/// lifecycle.
///
/// # Errors
///
/// If the address cannot be bound.
pub async fn server(
    config: Arc<ServerConfig>,
    controllers: fx::Group<ControllerPort>,
    life: fx::Lifecycle,
) -> std::io::Result<Server> {
    let router = controllers
        .iter()
        .fold(Router::new(), |router, controller| router.merge(controller.routes()));
    let listener = TcpListener::bind(config.addr).await?;
    let addr = listener.local_addr()?;

    let (stop, stopped) = oneshot::channel::<()>();
    let task: Arc<Mutex<Option<JoinHandle<std::io::Result<()>>>>> = Arc::default();

    life.on_start({
        let task = task.clone();
        move || async move {
            let handle = tokio::spawn(async move {
                axum::serve(listener, router)
                    .with_graceful_shutdown(async {
                        // Either a stop request or a dropped sender ends the server.
                        let _ = stopped.await;
                    })
                    .await
            });
            *task.lock().unwrap_or_else(PoisonError::into_inner) = Some(handle);
        }
    });
    life.on_stop(move || async move {
        let _ = stop.send(());
        let handle = task.lock().unwrap_or_else(PoisonError::into_inner).take();
        match handle {
            Some(handle) => handle.await.map_err(std::io::Error::other)?,
            None => Ok(()),
        }
    });
    Ok(Server { addr })
}
