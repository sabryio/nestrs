use std::sync::Arc;

use nestrs::prelude::*;
use uuid::Uuid;

use super::dto::{CreateUserRequest, UserResponse};
use super::idempotent::IdempotentWrites;
use crate::application::{GetUser, ListUsers, RegisterUser};
use nestrs::http::BearerAuth;
use nestrs_interceptors::{ETag, SecurityHeaders};
use nestrs_kit::{PaginationPipe, RequestContext, TimingInterceptor};

/// HTTP surface of the users use cases: translate, delegate, translate back.
#[injectable]
pub struct UsersController {
    register: Arc<RegisterUser>,
    get: Arc<GetUser>,
    list: Arc<ListUsers>,
}

#[controller("/users")]
#[interceptor(TimingInterceptor)]
#[interceptor(SecurityHeaders::default().without_hsts())]
#[interceptor(ETag::default())]
impl UsersController {
    /// List users.
    ///
    /// Paginated with `limit` and `offset`.
    #[get]
    async fn list(&self, Piped(pagination): Piped<PaginationPipe>) -> Result<Json<Vec<UserResponse>>, HttpError> {
        let users = self.list.execute(pagination.try_into()?).await?;
        Ok(Json(users.into_iter().map(UserResponse::from).collect()))
    }

    /// Fetch one user by id.
    #[get("/{id}")]
    #[response(404, "No user with this id")]
    async fn find(&self, Path(id): Path<Uuid>) -> Result<Json<UserResponse>, HttpError> {
        Ok(Json(self.get.execute(id).await?.into()))
    }

    /// Register a user. Emails are unique, case-insensitively. The response echoes
    /// the request id (the caller's `x-request-id`, or a generated one).
    #[post]
    #[guard(BearerAuth)]
    #[roles("admin")]
    #[interceptor(RateLimit::per_minute(60).by(KeyBy::Caller))]
    #[interceptor(IdempotentWrites)]
    #[status(201)]
    #[response(409, "The email is already registered")]
    #[response(429, "Too many writes; see the Retry-After header")]
    async fn create(
        &self,
        Inject(context): Inject<RequestContext>,
        ValidatedJson(request): ValidatedJson<CreateUserRequest>,
    ) -> Result<([(&'static str, String); 1], Json<UserResponse>), HttpError> {
        let user = self.register.execute(request.into()).await?;
        let request_id = context.request_id();
        tracing::info!(%request_id, user = %user.id(), "user registered");
        Ok(([("x-request-id", request_id)], Json(user.into())))
    }
}
