use nestrs::prelude::*;

struct NotAnInterceptor;

#[injectable]
struct C;

#[controller("/c")]
impl C {
    #[get]
    #[interceptor(NotAnInterceptor)]
    async fn handler(&self) {}
}

fn main() {}
