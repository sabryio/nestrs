use nestrs::http::axum::extract::Request;
use nestrs::http::axum::middleware::Next;
use nestrs::http::axum::response::Response;
use nestrs::prelude::*;
use nestrs_interceptors::{Idempotency, IdempotencyStorePort};

/// Makes a write safe to retry: attach it with `#[interceptor(IdempotentWrites)]`.
///
/// It is an injectable interceptor over the [`IdempotencyStorePort`], so the choice
/// between in-process and Redis storage is made once, in configuration, and the
/// controllers never know.
#[injectable]
pub struct IdempotentWrites {
    store: IdempotencyStorePort,
}

impl Interceptor for IdempotentWrites {
    async fn intercept(&self, request: Request, next: Next) -> Response {
        Idempotency::new(self.store.clone()).intercept(request, next).await
    }
}
