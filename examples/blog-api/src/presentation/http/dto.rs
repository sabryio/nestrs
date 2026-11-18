//! The wire format. Entities never cross the HTTP boundary directly: requests are
//! converted into use-case commands and entities into response DTOs, so the JSON
//! shape can evolve independently of the domain model.
//!
//! The `validator` rules give clients field-by-field feedback (422). The domain
//! enforces the same invariants independently, so use cases stay correct when
//! called from somewhere other than HTTP.

use purlin::openapi::ToSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use validator::Validate;

use crate::application::{PublishPostCommand, RegisterUserCommand};
use crate::domain::{Post, PostEvent, User};

/// Request body for `POST /users`.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct CreateUserRequest {
    /// Display name, 1–100 characters.
    #[validate(length(min = 1, max = 100, message = "must be between 1 and 100 characters"))]
    #[schema(min_length = 1, max_length = 100)]
    pub name: String,
    /// Email address; compared case-insensitively.
    #[validate(email(message = "must be a valid email address"))]
    #[schema(format = Email)]
    pub email: String,
}

impl From<CreateUserRequest> for RegisterUserCommand {
    fn from(request: CreateUserRequest) -> Self {
        Self {
            name: request.name,
            email: request.email,
        }
    }
}

/// A user as returned to clients.
#[derive(Debug, Serialize, ToSchema)]
pub struct UserResponse {
    /// Unique identifier.
    pub id: Uuid,
    /// Display name.
    pub name: String,
    /// Lower-cased email address.
    pub email: String,
}

impl From<User> for UserResponse {
    fn from(user: User) -> Self {
        Self {
            id: user.id(),
            name: user.name().as_str().to_owned(),
            email: user.email().as_str().to_owned(),
        }
    }
}

/// Request body for `POST /posts`.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct CreatePostRequest {
    /// Must be an existing user.
    pub author_id: Uuid,
    /// Headline, 1–200 characters.
    #[validate(length(min = 1, max = 200, message = "must be between 1 and 200 characters"))]
    #[schema(min_length = 1, max_length = 200)]
    pub title: String,
    /// Body text, at least one character.
    #[validate(length(min = 1, message = "must not be empty"))]
    #[schema(min_length = 1)]
    pub body: String,
}

impl From<CreatePostRequest> for PublishPostCommand {
    fn from(request: CreatePostRequest) -> Self {
        Self {
            author_id: request.author_id,
            title: request.title,
            body: request.body,
        }
    }
}

/// A post as returned to clients.
#[derive(Debug, Serialize, ToSchema)]
pub struct PostResponse {
    /// Unique identifier.
    pub id: Uuid,
    /// The author's user id.
    pub author_id: Uuid,
    /// Headline.
    pub title: String,
    /// Body text.
    pub body: String,
}

impl From<Post> for PostResponse {
    fn from(post: Post) -> Self {
        Self {
            id: post.id(),
            author_id: post.author_id(),
            title: post.title().as_str().to_owned(),
            body: post.body().as_str().to_owned(),
        }
    }
}

/// One event on `GET /posts/events`, discriminated by `type`.
///
/// This is the stream's contract: the handler returns `EventStream<PostEventDto>`,
/// so the OpenAPI document (and any generated client) knows every shape a message
/// can take.
#[derive(Debug, Serialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PostEventDto {
    /// A post was published.
    Published {
        /// The new post.
        post: PostResponse,
    },
    /// A post was deleted.
    Removed {
        /// Id of the deleted post.
        id: Uuid,
    },
}

impl From<PostEvent> for PostEventDto {
    fn from(event: PostEvent) -> Self {
        match event {
            PostEvent::Published(post) => Self::Published { post: post.into() },
            PostEvent::Removed(id) => Self::Removed { id },
        }
    }
}
