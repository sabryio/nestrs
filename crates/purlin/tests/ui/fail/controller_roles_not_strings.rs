use purlin::prelude::*;

#[injectable]
struct C;

#[controller("/c")]
impl C {
    #[get]
    #[permissions(posts_delete)]
    async fn handler(&self) {}
}

fn main() {}
