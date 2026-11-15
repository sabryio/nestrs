//! **Composition layer**: the one place that knows every other layer.
//!
//! Modules decide which adapter implements which port, which use cases exist, and
//! which controllers expose them. Outer rings are chosen here and nowhere else;
//! everything inside depends only on abstractions.

mod app;
mod auth_module;
mod persistence_module;
mod posts_module;
mod seed_module;
mod shared_state_module;
mod users_module;

pub use app::AppModule;
pub use auth_module::AuthModule;
pub use persistence_module::PersistenceModule;
pub use posts_module::PostsModule;
pub use seed_module::SeedModule;
pub use shared_state_module::SharedStateModule;
pub use users_module::UsersModule;
