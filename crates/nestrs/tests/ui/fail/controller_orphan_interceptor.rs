use nestrs::prelude::*;

#[injectable]
struct Timing;

impl Interceptor for Timing {
    async fn intercept(
        &self,
        request: nestrs::http::axum::extract::Request,
        next: nestrs::http::axum::middleware::Next,
    ) -> nestrs::http::axum::response::Response {
        next.run(request).await
    }
}

#[injectable]
struct C;

#[controller("/c")]
impl C {
    #[get]
    async fn handler(&self) {}

    #[interceptor(Timing)]
    async fn helper(&self) {}
}

fn main() {}
