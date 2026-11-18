use purlin::prelude::*;

#[injectable]
struct NotAPipe;

#[injectable]
struct C;

#[controller("/c")]
impl C {
    #[get]
    async fn handler(&self, Piped(_value): Piped<NotAPipe>) {}
}

fn main() {}
