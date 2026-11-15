//! HTTP controllers, DTOs and error mapping. Cross-cutting pieces (timing, pagination,
//! request context, error logging) come from `nestrs-kit`.

mod dto;
mod error;
mod idempotent;
mod page;
mod posts;
mod users;

pub use dto::{CreatePostRequest, CreateUserRequest, PostEventDto, PostResponse, UserResponse};
pub use idempotent::IdempotentWrites;
pub use posts::PostsController;
pub use users::UsersController;
