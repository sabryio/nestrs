# 2. The domain

The domain is plain Rust: no framework types, no I/O. Start with the smallest things.

## Values that cannot be wrong

A title is not a `String`; it is a `String` that has been checked. Parsing happens once, at the edge of the domain, and
everything after it can trust the value.

<!-- source: examples/blog-api/src/domain/post.rs -->
```rust
/// A headline of 1–200 characters, trimmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostTitle(String);

impl PostTitle {
    /// Validates and trims `raw`.
    pub fn parse(raw: &str) -> Result<Self, DomainError> {
        let trimmed = raw.trim();
        if (1..=200).contains(&trimmed.chars().count()) {
            Ok(Self(trimmed.to_owned()))
        } else {
            Err(DomainError::InvalidPostTitle)
        }
    }

    /// The trimmed title.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Non-empty body text, kept exactly as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostBody(String);

impl PostBody {
    /// Rejects bodies that are empty or only whitespace.
    pub fn parse(raw: &str) -> Result<Self, DomainError> {
        if raw.trim().is_empty() {
            Err(DomainError::EmptyPostBody)
        } else {
            Ok(Self(raw.to_owned()))
        }
    }

    /// The body text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
```

`PostTitle::parse` is the only way to build one, so a post with an empty title cannot exist anywhere in the program.

## Entities

<!-- source: examples/blog-api/src/domain/post.rs -->
```rust
/// A blog post.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Post {
    id: Uuid,
    author_id: Uuid,
    title: PostTitle,
    body: PostBody,
}

impl Post {
    /// Publishes a new post with a fresh id. The caller is responsible for having
    /// checked that `author_id` exists (that needs a repository, so it is a use case's job).
    pub fn publish(author_id: Uuid, title: PostTitle, body: PostBody) -> Self {
        Self {
            id: Uuid::new_v4(),
            author_id,
            title,
            body,
        }
    }

    /// Rebuilds a post from stored parts (used by repositories).
    pub fn restore(id: Uuid, author_id: Uuid, title: PostTitle, body: PostBody) -> Self {
        Self {
            id,
            author_id,
            title,
            body,
        }
    }

    /// Unique identifier.
    pub fn id(&self) -> Uuid {
        self.id
    }

    /// The author's user id.
    pub fn author_id(&self) -> Uuid {
        self.author_id
    }

    /// Headline.
    pub fn title(&self) -> &PostTitle {
        &self.title
    }

    /// Body text.
    pub fn body(&self) -> &PostBody {
        &self.body
    }
}
```

Two constructors with different jobs: `publish` creates a *new* post (fresh id), `restore` rebuilds a stored one for
repositories. The entity has no setters; behaviour is added as methods when the domain needs it.

## Errors in the domain's own words

<!-- source: examples/blog-api/src/domain/error.rs -->
```rust
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
```

Note what is absent: no HTTP status, no SQL error. Those translate at the edges (chapter 4).

## Ports

The domain needs to store posts but must not know how. It declares the need as a trait:

<!-- source: examples/blog-api/src/domain/repository.rs -->
```rust
/// Port: persistence for posts.
#[port]
pub trait PostRepository {
    /// Looks a post up by id.
    async fn find(&self, id: Uuid) -> Result<Option<Post>, RepositoryError>;
    /// One page of posts in a stable order.
    async fn list(&self, page: Page) -> Result<Vec<Post>, RepositoryError>;
    /// Stores a post.
    async fn insert(&self, post: Post) -> Result<(), RepositoryError>;
    /// Removes a post, returning whether it existed.
    async fn remove(&self, id: Uuid) -> Result<bool, RepositoryError>;
}
```

`#[port]` also generates `PostRepositoryPort` (an `Arc<dyn PostRepository>`), the handle other code injects. Read the
contract in the doc comment of `UserRepository` in the same file: *any* implementation is acceptable if it honours it. That
sentence is the Liskov substitution principle written as documentation, and the repository tests in later chapters hold
every adapter to it.

**SOLID here.** Value objects and entities keep *single responsibility*; `PostRepository` is a small, role-specific
interface (*interface segregation*: users and posts have separate ports); the port lives in the domain so the dependency
points inward (*dependency inversion*).
