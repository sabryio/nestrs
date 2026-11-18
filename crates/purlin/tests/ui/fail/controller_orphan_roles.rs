use purlin::prelude::*;

#[injectable]
struct C;

#[controller("/c")]
impl C {
    #[get]
    async fn handler(&self) {}

    #[roles("admin")]
    async fn helper(&self) {}
}

fn main() {}
