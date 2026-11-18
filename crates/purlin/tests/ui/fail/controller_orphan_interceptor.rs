use purlin::prelude::*;

#[injectable]
struct Timing;

impl Interceptor for Timing {
    async fn intercept(
        &self,
        request: purlin::http::axum::extract::Request,
        next: purlin::http::axum::middleware::Next,
    ) -> purlin::http::axum::response::Response {
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
