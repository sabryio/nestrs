use nestrs::prelude::*;

#[injectable]
struct C;

#[controller("/c")]
impl C {
    #[get]
    #[interceptor(42)]
    async fn handler(&self) {}
}

fn main() {}
