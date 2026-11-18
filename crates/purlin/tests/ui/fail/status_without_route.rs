use purlin::prelude::*;

#[injectable]
struct C;

#[controller("/c")]
impl C {
    #[get]
    async fn ok(&self) -> StatusCode {
        StatusCode::OK
    }

    #[status(201)]
    async fn helper(&self) {}
}

fn main() {}
