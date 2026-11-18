use purlin::prelude::*;

struct NotAnExtractor;

#[injectable]
struct C;

#[controller("/c")]
impl C {
    #[get]
    async fn handler(&self, _value: NotAnExtractor) {}
}

fn main() {}
