use nestrs::prelude::*;

#[injectable]
struct C;

#[controller("/c")]
impl C {
    #[get]
    async fn handler(&self, count: nestrs::http::axum::extract::State<Arc<u32>>) -> String {
        count.0.to_string()
    }
}

fn main() {}
