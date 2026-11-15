use nestrs::prelude::*;

#[injectable]
struct C;

#[controller("/c")]
impl C {
    #[get]
    #[response(201, "Created")]
    async fn nope(&self) -> StatusCode {
        StatusCode::OK
    }
}

fn main() {}
