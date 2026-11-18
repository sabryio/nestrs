use purlin::port;
use uuid::Uuid;

use super::{Email, Page, Post, RepositoryError, User};

/// Port: persistence for users.
///
/// Defined by the domain, implemented by infrastructure (the dependency points
/// inward). `#[port]` also generates `UserRepositoryPort`, the injectable handle. Any implementation is substitutable as long as it honours the contract:
/// lookups return `Ok(None)` for unknown keys, `insert` stores the user as given,
/// and `list` returns the requested window of a stable order.
#[port]
pub trait UserRepository {
    /// Looks a user up by id.
    async fn find(&self, id: Uuid) -> Result<Option<User>, RepositoryError>;
    /// Looks a user up by email.
    async fn find_by_email(&self, email: &Email) -> Result<Option<User>, RepositoryError>;
    /// One page of users in a stable order.
    async fn list(&self, page: Page) -> Result<Vec<User>, RepositoryError>;
    /// Stores a user.
    async fn insert(&self, user: User) -> Result<(), RepositoryError>;
}

/// Port: persistence for posts.
#[port]
pub trait PostRepository {
    /// Looks a post up by id.
    async fn find(&self, id: Uuid) -> Result<Option<Post>, RepositoryError>;
    /// One page of posts in a stable order.
    async fn list(&self, page: Page) -> Result<Vec<Post>, RepositoryError>;
    /// Stores a post.
    async fn insert(&self, post: Post) -> Result<(), RepositoryError>;
    /// Removes a post, returning whether it existed.
    async fn remove(&self, id: Uuid) -> Result<bool, RepositoryError>;
}
