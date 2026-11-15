use uuid::Uuid;

use crate::domain::{DomainError, RepositoryError};

/// Everything a use case can fail with. One enum for the layer: the presentation
/// layer maps it to a transport-specific error in exactly one place.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ApplicationError {
    /// Input violated a domain invariant.
    #[error(transparent)]
    Invalid(#[from] DomainError),
    /// No user has this id.
    #[error("user {0} not found")]
    UserNotFound(Uuid),
    /// No post has this id.
    #[error("post {0} not found")]
    PostNotFound(Uuid),
    /// A post referenced an author that does not exist.
    #[error("author {0} does not exist")]
    AuthorNotFound(Uuid),
    /// Another user already registered this email.
    #[error("email `{0}` is already registered")]
    EmailTaken(String),
    /// Storage failed.
    #[error(transparent)]
    Repository(#[from] RepositoryError),
}
