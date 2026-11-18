# 3. Use cases

A use case is one thing the application can do. It orchestrates: validate input with the domain, call ports, return a
domain value or a typed error. It contains no HTTP and no SQL.

<!-- source: examples/blog-api/src/application/posts.rs -->
```rust
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
```

Three things to notice:

1. **Dependencies are ports.** The struct's fields are `PostRepositoryPort`, `UserRepositoryPort`, and so on. Nothing says
   which adapter; `#[injectable]` makes the container supply whatever the composition root bound (chapter 5).
2. **One entry point.** `execute` takes a command struct (`PublishPostCommand`), so the signature stays stable as inputs grow.
3. **Use cases do not call each other.** `PublishPost` depends on the user repository *port*, not on the users use cases.
   Ports are the building blocks; shared logic goes in the domain.

Smaller ones follow the same shape:

<!-- source: examples/blog-api/src/application/posts.rs -->
```rust
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
```

## One error type for the layer

<!-- source: examples/blog-api/src/application/error.rs -->
```rust
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
```

`#[from]` lets a use case write `?` on a domain or repository error and get the right variant.

## Add one with the CLI

```text
purlin generate use-case post archive
```

creates `ArchivePost` with a unit test and a fake repository, exports it from `application/`, and adds it to the feature
module's `providers`. `--dry-run` shows the diff first.

## Testing without a framework

Because dependencies are traits, a use case is tested by handing it fakes (`application/testing.rs` has in-memory ones),
with no server and no database. That is the payoff for the discipline of chapters 1 and 2.

**SOLID here.** `PublishPost` does one job (*single responsibility*); you add behaviour by adding use cases and adapters
rather than editing existing ones (*open/closed*); it depends on abstractions only (*dependency inversion*).
