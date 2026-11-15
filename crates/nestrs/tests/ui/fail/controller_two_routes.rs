use nestrs::prelude::*;

#[injectable]
struct C;

#[controller("/c")]
impl C {
    #[get]
    #[post]
    async fn handler(&self) {}
}

fn main() {}
