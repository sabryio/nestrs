use nestrs::port;

use super::{Post, RepositoryError};

/// Port: tells the rest of the world that something happened, **reliably**.
///
/// Unlike [`PostEvents`](super::PostEvents) (a live feed that may drop what nobody is
/// listening to), an announcement must not be lost and must not be sent for something that
/// did not happen. The adapter therefore records it inside the same transaction as the write
/// that caused it, and delivery happens afterwards, at least once. Consumers deduplicate.
#[port]
pub trait PostAnnouncements {
    /// Records that `post` was published. Called after the post is stored; if this fails
    /// the whole operation fails and nothing is kept.
    async fn post_published(&self, post: &Post) -> Result<(), RepositoryError>;
}
