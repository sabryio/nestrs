use std::sync::Arc;

use purlin::prelude::*;
use uuid::Uuid;

use super::dto::{CreatePostRequest, PostEventDto, PostResponse};
use super::idempotent::IdempotentWrites;
use super::page::to_page;
use crate::application::{DeletePost, GetPost, ListPosts, PublishPost, WatchPosts};
use crate::domain::{EventCursor, Sequenced};
use purlin::http::BearerAuth;
use purlin_interceptors::{ETag, SecurityHeaders};
use purlin_kit::{PaginationPipe, TimingInterceptor};
use purlin_sea_orm::Transactional;

/// HTTP surface of the posts use cases. Reads are public; writes need a token.
#[injectable]
pub struct PostsController {
    publish: Arc<PublishPost>,
    get: Arc<GetPost>,
    list: Arc<ListPosts>,
    delete: Arc<DeletePost>,
    watch: Arc<WatchPosts>,
}

#[controller("/posts")]
#[interceptor(TimingInterceptor)]
#[interceptor(SecurityHeaders::default().without_hsts())]
#[interceptor(ETag::default())]
impl PostsController {
    /// List posts.
    ///
    /// Paginated with `limit` and `offset`.
    #[get]
    async fn list(&self, Piped(pagination): Piped<PaginationPipe>) -> Result<Json<Vec<PostResponse>>, HttpError> {
        let posts = self.list.execute(to_page(pagination)?).await?;
        Ok(Json(posts.into_iter().map(PostResponse::from).collect()))
    }

    /// Stream post events.
    ///
    /// Server-Sent Events: a flush comment, then one message per event
    /// (`published` or `removed`) each with an `id`, then `event: close` when the
    /// server stops. Reconnect with a `Last-Event-ID` header to resume after that
    /// event: what was missed is replayed first, then the stream goes live.
    #[get("/events")]
    async fn events(&self, last: LastEventId) -> EventStream<PostEventDto> {
        // A missing or malformed id is not an error: the client just gets a live stream.
        let subscription = self.watch.execute(last.parse::<EventCursor>()).await;
        EventStream::from_events(purlin::http::stream::unfold(subscription, |mut feed| async move {
            let Sequenced { cursor, event } = feed.next().await?;
            Some((Ok(Event::with_id(cursor, PostEventDto::from(event))), feed))
        }))
    }

    /// Fetch one post by id.
    #[get("/{id}")]
    #[response(404, "No post with this id")]
    async fn find(&self, Path(id): Path<Uuid>) -> Result<Json<PostResponse>, HttpError> {
        Ok(Json(self.get.execute(id).await?.into()))
    }

    /// Publish a post as an existing author.
    #[post]
    #[guard(BearerAuth)]
    #[roles("admin", "author")]
    #[interceptor(RateLimit::per_minute(60).by(KeyBy::Caller))]
    #[interceptor(IdempotentWrites)]
    #[interceptor(Transactional)]
    #[status(201)]
    #[response(404, "The author does not exist")]
    #[response(429, "Too many writes; see the Retry-After header")]
    async fn create(
        &self,
        ValidatedJson(request): ValidatedJson<CreatePostRequest>,
    ) -> Result<Json<PostResponse>, HttpError> {
        let post = self.publish.execute(request.into()).await?;
        Ok(Json(post.into()))
    }

    /// Delete a post.
    #[delete("/{id}")]
    #[guard(BearerAuth)]
    #[permissions("posts:delete")]
    #[interceptor(Transactional)]
    #[status(204)]
    #[response(404, "No post with this id")]
    async fn remove(&self, Path(id): Path<Uuid>) -> Result<(), HttpError> {
        self.delete.execute(id).await?;
        Ok(())
    }
}
