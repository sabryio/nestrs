use purlin::prelude::*;

#[injectable]
struct C;

#[controller("/c")]
impl C {
    #[get]
    #[status(404)]
    async fn nope(&self) -> StatusCode {
        StatusCode::OK
    }
}

fn main() {}
