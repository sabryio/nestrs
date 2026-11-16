//! What flows through the outbox.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::OutboxError;

/// A message to be recorded. Build it with [`NewMessage::json`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMessage {
    /// Where it should go: a queue name, a stream, a topic. The publisher decides what it means.
    pub topic: String,
    /// Messages with the same key are delivered one at a time, in the order they were
    /// recorded. Use an aggregate id (`"order-42"`). `None` means no ordering is needed.
    pub key: Option<String>,
    /// The JSON payload.
    pub payload: String,
}

impl NewMessage {
    /// Serialises `payload` as the message body.
    pub fn json<T: Serialize>(topic: &str, key: Option<&str>, payload: &T) -> Result<Self, OutboxError> {
        Ok(Self {
            topic: topic.to_owned(),
            key: key.map(str::to_owned),
            payload: serde_json::to_string(payload).map_err(|error| OutboxError::Encode(error.to_string()))?,
        })
    }
}

/// A recorded message as the relay sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxMessage {
    /// Unique and stable across retries: the idempotency key downstream consumers dedup on.
    pub id: String,
    /// Destination.
    pub topic: String,
    /// Ordering key, if any.
    pub key: Option<String>,
    /// The JSON payload.
    pub payload: String,
    /// Delivery attempts that already failed.
    pub attempts: u32,
    /// When it was recorded, in milliseconds since the Unix epoch.
    pub created_at_ms: i64,
    /// Opaque proof that the holder claimed this message; stores use it to ignore a late
    /// `complete` from a relay whose claim expired.
    pub claim: String,
}

/// The shape [`QueuePublisher`](crate::QueuePublisher) puts on a queue, so a handler gets the
/// message id (to deduplicate redeliveries) along with the payload.
///
/// ```
/// # use nestrs_outbox::OutboxEnvelope;
/// let envelope: OutboxEnvelope<u32> = serde_json::from_str(r#"{"id":"a","topic":"t","payload":7}"#).unwrap();
/// assert_eq!(envelope.payload, 7);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboxEnvelope<T> {
    /// The outbox message id: identical on every redelivery.
    pub id: String,
    /// The topic it was recorded under.
    pub topic: String,
    /// The recorded payload.
    pub payload: T,
}

impl<T: DeserializeOwned> OutboxEnvelope<T> {
    /// Parses an envelope from a queue payload.
    pub fn parse(payload: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(payload)
    }
}
