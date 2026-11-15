use std::future::Future;
use std::pin::Pin;

use nestrs::prelude::*;
use nestrs_redis::{RedisConnection, RedisEventStream, StreamId, StreamSubscription};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::{EventCursor, Post, PostBody, PostEvent, PostEvents, PostSubscription, PostTitle, Sequenced};

/// How many events Redis keeps for resuming clients (about; the stream is trimmed lazily).
const RETAINED: usize = 10_000;

/// [`PostEvents`] over a Redis Stream: every instance publishes into, and every SSE client
/// reads from, one shared, ordered feed.
///
/// A client connected to instance B sees a post published on instance A, and a `Last-Event-ID`
/// from one instance resumes on another, because the cursor is Redis's own stream id. Events
/// are stored as JSON in a wire format owned by this adapter, so the domain types stay free of serde.
pub struct RedisPostEvents {
    stream: RedisEventStream,
}

impl RedisPostEvents {
    /// Connects the feed on the stream `key` and starts its reader.
    pub async fn connect(redis: RedisConnection, key: &str) -> Result<Self, nestrs_redis::RedisError> {
        Ok(Self {
            stream: RedisEventStream::new(redis, key, RETAINED).await?,
        })
    }
}

/// What goes into Redis. Versioned by shape: add fields, never repurpose them.
#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Wire {
    Published {
        id: Uuid,
        author_id: Uuid,
        title: String,
        body: String,
    },
    Removed {
        id: Uuid,
    },
}

impl From<&PostEvent> for Wire {
    fn from(event: &PostEvent) -> Self {
        match event {
            PostEvent::Published(post) => Wire::Published {
                id: post.id(),
                author_id: post.author_id(),
                title: post.title().as_str().to_owned(),
                body: post.body().as_str().to_owned(),
            },
            PostEvent::Removed(id) => Wire::Removed { id: *id },
        }
    }
}

impl Wire {
    /// Back to the domain; `None` when the stored values no longer satisfy the domain's rules.
    fn into_event(self) -> Option<PostEvent> {
        Some(match self {
            Wire::Published {
                id,
                author_id,
                title,
                body,
            } => PostEvent::Published(Post::restore(
                id,
                author_id,
                PostTitle::parse(&title).ok()?,
                PostBody::parse(&body).ok()?,
            )),
            Wire::Removed { id } => PostEvent::Removed(id),
        })
    }
}

#[async_trait]
impl PostEvents for RedisPostEvents {
    fn publish(&self, event: PostEvent) {
        match serde_json::to_vec(&Wire::from(&event)) {
            Ok(payload) => self.stream.publish_detached(payload),
            Err(error) => tracing::error!(%error, "could not encode a post event"),
        }
    }

    async fn subscribe(&self, after: Option<EventCursor>) -> Box<dyn PostSubscription> {
        let after = after.map(|cursor| StreamId {
            ms: cursor.epoch(),
            seq: cursor.seq(),
        });
        match self.stream.subscribe(after).await {
            Ok(inner) => Box::new(Subscription { inner: Some(inner) }),
            Err(error) => {
                // The port cannot fail; a client that cannot be served now gets an ended stream
                // and reconnects with its Last-Event-ID, which is what SSE clients do anyway.
                tracing::error!(%error, "could not subscribe to the event stream");
                Box::new(Subscription { inner: None })
            }
        }
    }
}

struct Subscription {
    inner: Option<StreamSubscription>,
}

impl PostSubscription for Subscription {
    fn next(&mut self) -> Pin<Box<dyn Future<Output = Option<Sequenced>> + Send + '_>> {
        Box::pin(async move {
            let inner = self.inner.as_mut()?;
            loop {
                let entry = inner.next().await?;
                let decoded = serde_json::from_slice::<Wire>(&entry.payload)
                    .ok()
                    .and_then(Wire::into_event);
                match decoded {
                    Some(event) => {
                        return Some(Sequenced {
                            cursor: EventCursor::new(entry.id.ms, entry.id.seq),
                            event,
                        });
                    }
                    // A bad entry must not end every client's stream: skip it and say so.
                    None => tracing::warn!(id = %entry.id, "skipping an undecodable event"),
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_survive_the_wire_and_bad_values_are_rejected() {
        let post = Post::publish(
            Uuid::new_v4(),
            PostTitle::parse("Hello").unwrap(),
            PostBody::parse("World").unwrap(),
        );
        for event in [PostEvent::Published(post), PostEvent::Removed(Uuid::new_v4())] {
            let json = serde_json::to_vec(&Wire::from(&event)).unwrap();
            let back = serde_json::from_slice::<Wire>(&json).unwrap().into_event();
            assert_eq!(back, Some(event));
        }
        let bad = Wire::Published {
            id: Uuid::nil(),
            author_id: Uuid::nil(),
            title: String::new(),
            body: "x".into(),
        };
        assert!(
            bad.into_event().is_none(),
            "the domain's rules still apply to stored data"
        );
    }
}
