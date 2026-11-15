use uuid::Uuid;

/// A business invariant was violated while building a domain value.
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum DomainError {
    /// The email address is not well formed.
    #[error("`{0}` is not a valid email address")]
    InvalidEmail(String),
    /// The name is empty or too long.
    #[error("user name must be between 1 and 100 characters")]
    InvalidUserName,
    /// The title is empty or too long.
    #[error("post title must be between 1 and 200 characters")]
    InvalidPostTitle,
    /// The requested page size is outside the allowed range.
    #[error("page limit must be between 1 and {max}")]
    InvalidPage {
        /// The largest allowed limit.
        max: u64,
    },
    /// The body is empty.
    #[error("post body must not be empty")]
    EmptyPostBody,
}

/// A repository could not complete an operation (connection lost, timeout, ..).
///
/// Ports return this instead of panicking or leaking a driver's error type, so
/// use cases can stay ignorant of the storage technology.
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum RepositoryError {
    /// The backing store is unreachable or failed.
    #[error("storage unavailable: {0}")]
    Unavailable(String),
    /// A stored record could not be turned back into a valid entity.
    #[error("record {id} is corrupt: {reason}")]
    Corrupt {
        /// The offending record.
        id: Uuid,
        /// Why it was rejected.
        reason: String,
    },
}
