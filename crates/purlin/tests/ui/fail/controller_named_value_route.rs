use purlin::prelude::*;

#[injectable]
struct C;

#[controller("/c")]
impl C {
    #[get = "/x"]
    async fn handler(&self) {}
}

fn main() {}
