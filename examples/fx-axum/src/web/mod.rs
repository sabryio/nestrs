//! The HTTP side: controllers contribute routes; the server serves them.

use axum::Router;

pub mod greetings;
pub mod health;
pub mod server;

/// Something that contributes routes. A port: the server knows only this trait, so a new
/// controller needs no change to it. Controllers join the group with `#[fx::bind(group)]`.
#[fx::port]
pub trait Controller {
    /// This controller's routes.
    fn routes(&self) -> Router;
}
