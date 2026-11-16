//! Adapters for [`PostAnnouncements`] and the consumer of what they announce.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use nestrs::prelude::*;
use nestrs_jobs::{HandlerError, QueueHandler, QueueHandlerPort, decode};
use nestrs_outbox::{Outbox, OutboxEnvelope};
use nestrs_sea_orm::Database;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::{Post, PostAnnouncements, RepositoryError};

/// The topic (and queue) post announcements travel on.
pub const POST_PUBLISHED: &str = "post.published";

/// What an announcement says. Plain data: consumers need no domain types.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PostPublished {
    /// The post.
    pub post_id: Uuid,
    /// Who wrote it.
    pub author_id: Uuid,
    /// Its headline, for human-readable notifications.
    pub title: String,
}

/// Memory mode has no database to be consistent with, so there is nothing to record.
pub struct NoAnnouncements;

#[async_trait]
impl PostAnnouncements for NoAnnouncements {
    async fn post_published(&self, _post: &Post) -> Result<(), RepositoryError> {
        Ok(())
    }
}

/// Records the announcement in the outbox, through the request's executor: it commits or
/// rolls back together with the post itself.
pub struct OutboxPostAnnouncements {
    database: Arc<Database>,
}

impl OutboxPostAnnouncements {
    /// An adapter over the application's database.
    pub fn new(database: Arc<Database>) -> Self {
        Self { database }
    }
}

#[async_trait]
impl PostAnnouncements for OutboxPostAnnouncements {
    async fn post_published(&self, post: &Post) -> Result<(), RepositoryError> {
        let unavailable = |error: &dyn std::fmt::Display| RepositoryError::Unavailable(error.to_string());
        let db = self.database.executor().await.map_err(|e| unavailable(&e))?;
        let announcement = PostPublished {
            post_id: post.id(),
            author_id: post.author_id(),
            title: post.title().as_str().to_owned(),
        };
        // Keyed by post, so announcements about one post arrive in order.
        Outbox
            .record_json(&db, POST_PUBLISHED, Some(&post.id().to_string()), &announcement)
            .await
            .map(|_| ())
            .map_err(|e| unavailable(&e))
    }
}

/// What has been announced so far. Redeliveries (the outbox is at-least-once) are recognised
/// by message id and counted once.
#[injectable]
pub struct Announced {
    #[inject(default)]
    seen: Mutex<(HashSet<String>, Vec<PostPublished>)>,
}

impl Announced {
    /// Remembers an announcement; `false` if this message id was already handled.
    pub fn record(&self, message_id: &str, announcement: PostPublished) -> bool {
        let mut seen = self.seen.lock().unwrap_or_else(|p| p.into_inner());
        if !seen.0.insert(message_id.to_owned()) {
            return false;
        }
        seen.1.push(announcement);
        true
    }

    /// Everything announced, oldest first.
    pub fn all(&self) -> Vec<PostPublished> {
        self.seen.lock().unwrap_or_else(|p| p.into_inner()).1.clone()
    }
}

/// Consumes post announcements. A real service would send the notification here; the example
/// logs it and remembers it.
#[injectable(contributes = QueueHandlerPort)]
pub struct PostPublishedHandler {
    announced: Arc<Announced>,
}

#[async_trait]
impl QueueHandler for PostPublishedHandler {
    fn queue(&self) -> &str {
        POST_PUBLISHED
    }

    async fn handle(&self, payload: &str) -> Result<(), HandlerError> {
        let envelope: OutboxEnvelope<PostPublished> = decode(payload)?;
        let title = envelope.payload.title.clone();
        if self.announced.record(&envelope.id, envelope.payload) {
            tracing::info!(message = %envelope.id, %title, "post announced");
        }
        Ok(())
    }
}
