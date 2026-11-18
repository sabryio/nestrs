use purlin::prelude::*;

#[injectable]
struct C;

#[controller("/c")]
impl C {
    #[get]
    async fn handler(&self, count: purlin::http::axum::extract::State<Arc<u32>>) -> String {
        count.0.to_string()
    }
}

fn main() {}
