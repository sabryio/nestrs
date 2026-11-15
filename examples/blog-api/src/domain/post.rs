use uuid::Uuid;

use super::DomainError;

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
