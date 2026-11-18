//! Where messages go: the publisher port and the queue adapter.

use purlin::prelude::*;

use crate::OutboxMessage;

/// Delivery failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PublishError {
    /// Worth retrying: the destination was unreachable or refused for now.
    #[error("{0}")]
    Retry(String),
    /// Retrying cannot help (the destination rejects this message); it becomes a dead letter.
    #[error("{0}")]
    Fatal(String),
}

/// Port: delivers one message to its destination.
///
/// Delivery is **at least once**: if the relay stops after `publish` returned but before the
/// store recorded it, the message is published again. Make consumers idempotent on
/// [`OutboxMessage::id`], which is the same on every redelivery.
///
/// A publisher may serve several topics and route on `message.topic`.
#[port]
pub trait OutboxPublisher {
    /// Delivers `message`.
    async fn publish(&self, message: &OutboxMessage) -> Result<(), PublishError>;
}

/// Delivers to a [`purlin-jobs`](purlin_jobs) queue named after the topic, wrapped in an
/// [`OutboxEnvelope`](crate::OutboxEnvelope) so the handler sees the message id.
#[cfg(feature = "jobs")]
#[injectable(provides = OutboxPublisherPort)]
pub struct QueuePublisher {
    queue: Arc<purlin_jobs::Queue>,
}

#[cfg(feature = "jobs")]
#[async_trait]
impl OutboxPublisher for QueuePublisher {
    async fn publish(&self, message: &OutboxMessage) -> Result<(), PublishError> {
        let payload: serde_json::Value = serde_json::from_str(&message.payload)
            .map_err(|error| PublishError::Fatal(format!("the payload is not JSON: {error}")))?;
        let envelope = crate::OutboxEnvelope {
            id: message.id.clone(),
            topic: message.topic.clone(),
            payload,
        };
        self.queue
            .enqueue(&message.topic, &envelope)
            .await
            .map(|_| ())
            .map_err(|error| match error {
                purlin_jobs::QueueError::Backend(reason) => PublishError::Retry(reason),
                purlin_jobs::QueueError::Encode(reason) => PublishError::Fatal(reason),
            })
    }
}
