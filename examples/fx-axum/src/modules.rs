//! The fx modules and the composition root.
//!
//! Each module owns one concern and exports only its public surface:
//!
//! - `TimeModule` exports the clock port;
//! - `GreetingModule` exports the service (it keeps its store private);
//! - `WebModule` hosts the controllers and the server, and sees what it imports.

use crate::domain::{ClockPort, GreetingService};
use crate::infra::{InMemoryStore, SystemClock};
use crate::web::greetings::GreetingController;
use crate::web::health::HealthController;
use crate::web::server::{Server, ServerConfig, server};

/// Time.
#[fx::module(providers = [SystemClock], exports = [ClockPort])]
pub struct TimeModule;

/// The greeting use case and its storage.
#[fx::module(
    imports = [TimeModule],
    providers = [InMemoryStore, GreetingService::new],
    exports = [GreetingService],
)]
pub struct GreetingModule;

/// HTTP: controllers (a group) and the server.
#[fx::module(
    imports = [GreetingModule],
    providers = [GreetingController, HealthController, server],
    exports = [Server],
)]
pub struct WebModule;

/// The whole application. `config` says where to listen.
pub fn app(config: ServerConfig) -> fx::App {
    fx::App::new()
        .supply(config)
        .module::<TimeModule>()
        .module::<GreetingModule>()
        .module::<WebModule>()
        // The server is built (and so started) because an invoke needs it.
        .invoke(|_: std::sync::Arc<Server>| {})
}
