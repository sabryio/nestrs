//! Errors.

/// The store could not do what was asked.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("outbox store unavailable: {0}")]
pub struct StoreError(pub String);

/// Recording a message failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OutboxError {
    /// The payload could not be serialised.
    #[error("cannot encode the payload: {0}")]
    Encode(String),
    /// The database refused the write. The surrounding transaction should be rolled back.
    #[error("cannot record the message: {0}")]
    Store(String),
}

impl From<StoreError> for OutboxError {
    fn from(error: StoreError) -> Self {
        Self::Store(error.0)
    }
}
