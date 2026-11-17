# 4. HTTP

The presentation layer translates: JSON in, a command out; a domain value in, JSON out; a failure in, a status out.

## Requests and responses are not entities

<!-- source: examples/blog-api/src/presentation/http/dto.rs -->
```rust
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
```

`#[validate]` rejects malformed input with a 422 before the handler runs, and `ToSchema` puts the shape into the OpenAPI
document. DTOs convert to commands and from entities explicitly (`impl From<..>`), so the wire format can change without
touching the domain.

## One place for errors

<!-- source: examples/blog-api/src/presentation/http/error.rs -->
```rust
/// The single place where application failures meet HTTP status codes.
impl From<ApplicationError> for HttpError {
    fn from(error: ApplicationError) -> Self {
        match error {
            ApplicationError::Invalid(_) => Self::bad_request(error.to_string()),
            ApplicationError::UserNotFound(_) | ApplicationError::PostNotFound(_) => Self::not_found(error.to_string()),
            ApplicationError::AuthorNotFound(_) => Self::bad_request(error.to_string()),
            ApplicationError::EmailTaken(_) => Self::conflict(error.to_string()),
            // Storage details are logged by `HttpError`, never sent to the client.
            ApplicationError::Repository(_) => Self::internal(error.to_string()),
        }
    }
}
```

Every use case failure becomes a status here and nowhere else. Storage details are logged, never sent to the client.

## The controller

<!-- source: examples/blog-api/src/presentation/http/posts.rs -->
```rust
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
```

A handler is a few lines: extract, call the use case, convert. The attributes are the policy:

* `#[guard(BearerAuth)]` authenticates; `#[roles(..)]` and `#[permissions(..)]` authorise.
* `#[interceptor(..)]` wraps the handler: rate limiting, idempotency keys, a database transaction (chapter 6).
* `#[status(201)]` and `#[response(404, "..")]` are documentation *and* behaviour: they appear in OpenAPI and in the
  generated client's types (chapter 9).

The doc comments on handlers become the OpenAPI summaries, so write them for the people who will read the API reference.

**SOLID here.** The controller only translates (*single responsibility*); new endpoints are new methods and new use cases,
and cross-cutting behaviour is added with interceptors instead of editing handlers (*open/closed*).
