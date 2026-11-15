use nestrs::prelude::*;

#[injectable]
struct C;

#[controller("/c")]
impl C {
    #[get]
    async fn handler() {}
}

fn main() {}
