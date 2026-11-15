use nestrs::prelude::*;

struct NotAGuard;

#[injectable]
struct C;

#[controller("/c")]
impl C {
    #[get]
    #[guard(NotAGuard)]
    async fn handler(&self) {}
}

fn main() {}
