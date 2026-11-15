//! **Domain layer**: entities, value objects and the ports the domain needs.
//!
//! The innermost ring. It depends on nothing but `std` and tiny utility crates: no
//! web framework, no DI container, no serialisation format. Everything else in the
//! application depends on this module, never the reverse (enforced by
//! `tests/architecture.rs`).

mod error;
mod events;
mod page;
mod post;
mod repository;
mod user;

pub use error::{DomainError, RepositoryError};
pub use events::{EventCursor, InvalidCursor, PostEvent, PostEvents, PostEventsPort, PostSubscription, Sequenced};
pub use page::Page;
pub use post::{Post, PostBody, PostTitle};
pub use repository::{PostRepository, PostRepositoryPort, UserRepository, UserRepositoryPort};
pub use user::{Email, User, UserName};
