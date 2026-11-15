//! **Application layer**: the use cases.
//!
//! Each use case is a small struct holding the ports it needs and one `execute`
//! method. It orchestrates domain objects and ports; it knows nothing about HTTP,
//! JSON or any concrete storage, and touches the DI framework only through two
//! declarative attributes (`#[port]`, `#[injectable]`). Depends on `domain` only.

mod error;
mod posts;
mod users;

#[cfg(test)]
mod testing;

pub use error::ApplicationError;
pub use posts::{DeletePost, GetPost, ListPosts, PublishPost, PublishPostCommand, WatchPosts};
pub use users::{GetUser, ListUsers, RegisterUser, RegisterUserCommand};
