use nestrs::prelude::*;

#[injectable]
struct C;

#[controller("/c")]
impl C {
    #[get]
    async fn handler(&mut self) {}
}

fn main() {}
