use purlin::prelude::*;

#[injectable]
struct C;

#[controller("/c")]
impl C {
    #[get]
    #[roles()]
    async fn handler(&self) {}
}

fn main() {}
