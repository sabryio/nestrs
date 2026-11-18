use purlin::injectable;
use uuid::Uuid;

use super::ApplicationError;
use crate::domain::{
    EventCursor, Page, Post, PostAnnouncementsPort, PostBody, PostEvent, PostEventsPort, PostRepositoryPort,
    PostSubscription, PostTitle, UserRepositoryPort,
};

/// Input of [`PublishPost`].
#[derive(Debug, Clone)]
pub struct PublishPostCommand {
    /// Must be an existing user.
    pub author_id: Uuid,
    /// Headline.
    pub title: String,
    /// Body text.
    pub body: String,
}

/// Use case: publish a post on behalf of an existing author.
///
/// Depends on the *user repository port*, not on the users use cases: use cases are
/// not building blocks for each other, ports are.
#[injectable]
pub struct PublishPost {
    posts: PostRepositoryPort,
    users: UserRepositoryPort,
    events: PostEventsPort,
    announcements: PostAnnouncementsPort,
}

impl PublishPost {
    /// Validates the input, checks the author exists and stores the post.
    pub async fn execute(&self, command: PublishPostCommand) -> Result<Post, ApplicationError> {
        let title = PostTitle::parse(&command.title)?;
        let body = PostBody::parse(&command.body)?;
        if self.users.find(command.author_id).await?.is_none() {
            return Err(ApplicationError::AuthorNotFound(command.author_id));
        }
        let post = Post::publish(command.author_id, title, body);
        self.posts.insert(post.clone()).await?;
        self.announcements.post_published(&post).await?;
        self.events.publish(PostEvent::Published(post.clone()));
        Ok(post)
    }
}

/// Use case: fetch one post.
#[injectable]
pub struct GetPost {
    posts: PostRepositoryPort,
}

impl GetPost {
    /// Returns the post, or [`ApplicationError::PostNotFound`].
    pub async fn execute(&self, id: Uuid) -> Result<Post, ApplicationError> {
        self.posts.find(id).await?.ok_or(ApplicationError::PostNotFound(id))
    }
}

/// Use case: list all posts.
#[injectable]
pub struct ListPosts {
    posts: PostRepositoryPort,
}

impl ListPosts {
    /// One page of posts in the repository's stable order.
    pub async fn execute(&self, page: Page) -> Result<Vec<Post>, ApplicationError> {
        Ok(self.posts.list(page).await?)
    }
}

/// Use case: delete a post.
#[injectable]
pub struct DeletePost {
    posts: PostRepositoryPort,
    events: PostEventsPort,
}

impl DeletePost {
    /// Deletes the post, or reports [`ApplicationError::PostNotFound`].
    pub async fn execute(&self, id: Uuid) -> Result<(), ApplicationError> {
        if self.posts.remove(id).await? {
            self.events.publish(PostEvent::Removed(id));
            Ok(())
        } else {
            Err(ApplicationError::PostNotFound(id))
        }
    }
}

/// Use case: follow post events as they happen.
#[injectable]
pub struct WatchPosts {
    events: PostEventsPort,
}

impl WatchPosts {
    /// A new subscription. With a cursor it first replays what the caller missed
    /// since that position (best effort, see [`PostEvents`](crate::domain::PostEvents)),
    /// then continues live; without one it sees events from now on.
    pub async fn execute(&self, after: Option<EventCursor>) -> Box<dyn PostSubscription> {
        self.events.subscribe(after).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::application::testing::{FakeAnnouncements, FakeEvents, FakePosts, FakeUsers};
    use crate::domain::{DomainError, Email, PostEvent, User, UserName, UserRepository};

    async fn seeded() -> (Arc<FakeUsers>, Arc<FakePosts>, User) {
        let users = Arc::new(FakeUsers::default());
        let author = User::register(
            UserName::parse("Ada").unwrap(),
            Email::parse("ada@example.com").unwrap(),
        );
        users.insert(author.clone()).await.unwrap();
        (users, Arc::new(FakePosts::default()), author)
    }

    fn command(author_id: Uuid, title: &str, body: &str) -> PublishPostCommand {
        PublishPostCommand {
            author_id,
            title: title.to_owned(),
            body: body.to_owned(),
        }
    }

    #[tokio::test]
    async fn publishes_trims_titles_and_supports_the_full_lifecycle() {
        let (users, posts, author) = seeded().await;
        let events = Arc::new(FakeEvents::default());
        let post = PublishPost {
            posts: posts.clone(),
            users,
            events: events.clone(),
            announcements: Arc::new(FakeAnnouncements::default()),
        }
        .execute(command(author.id(), "  Hello  ", "World"))
        .await
        .unwrap();
        assert_eq!(post.title().as_str(), "Hello");
        assert_eq!(GetPost { posts: posts.clone() }.execute(post.id()).await.unwrap(), post);
        assert_eq!(
            ListPosts { posts: posts.clone() }
                .execute(Page::default())
                .await
                .unwrap()
                .len(),
            1
        );

        let delete = DeletePost {
            posts: posts.clone(),
            events: events.clone(),
        };
        delete.execute(post.id()).await.unwrap();
        assert_eq!(
            delete.execute(post.id()).await.unwrap_err(),
            ApplicationError::PostNotFound(post.id())
        );
        // Exactly one event per real change; the failed delete announced nothing.
        assert_eq!(
            events.published(),
            [PostEvent::Published(post.clone()), PostEvent::Removed(post.id())]
        );
    }

    #[tokio::test]
    async fn unknown_authors_are_rejected() {
        let (users, posts, _) = seeded().await;
        let ghost = Uuid::new_v4();
        let events = Arc::new(FakeEvents::default());
        let error = PublishPost {
            posts,
            users,
            events: events.clone(),
            announcements: Arc::new(FakeAnnouncements::default()),
        }
        .execute(command(ghost, "Hello", "World"))
        .await
        .unwrap_err();
        assert_eq!(error, ApplicationError::AuthorNotFound(ghost));
        assert!(events.published().is_empty());
    }

    #[tokio::test]
    async fn invalid_posts_are_rejected_before_any_lookup() {
        let (users, posts, author) = seeded().await;
        let publish = PublishPost {
            posts: posts.clone(),
            users,
            events: Arc::new(FakeEvents::default()),
            announcements: Arc::new(FakeAnnouncements::default()),
        };
        assert_eq!(
            publish.execute(command(author.id(), "", "body")).await.unwrap_err(),
            ApplicationError::Invalid(DomainError::InvalidPostTitle)
        );
        assert_eq!(
            publish.execute(command(author.id(), "title", "  ")).await.unwrap_err(),
            ApplicationError::Invalid(DomainError::EmptyPostBody)
        );
        assert!(ListPosts { posts }.execute(Page::default()).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_published_post_is_announced_and_a_failed_announcement_fails_the_publish() {
        let (users, posts, author) = seeded().await;
        let announcements = Arc::new(FakeAnnouncements::default());
        let events = Arc::new(FakeEvents::default());
        let publish = PublishPost {
            posts: posts.clone(),
            users: users.clone(),
            events: events.clone(),
            announcements: announcements.clone(),
        };
        let post = publish.execute(command(author.id(), "Hello", "World")).await.unwrap();
        assert_eq!(announcements.announced(), [post.id()]);

        let broken = PublishPost {
            posts,
            users,
            events: events.clone(),
            announcements: Arc::new(FakeAnnouncements::failing()),
        };
        assert!(matches!(
            broken.execute(command(author.id(), "Second", "Post")).await,
            Err(ApplicationError::Repository(
                crate::domain::RepositoryError::Unavailable(_)
            ))
        ));
        assert_eq!(events.published().len(), 1, "no live event for a publish that failed");
    }
}
