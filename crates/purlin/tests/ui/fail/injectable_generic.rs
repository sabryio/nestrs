use purlin::prelude::*;

#[injectable]
struct Service<T> {
    inner: Arc<T>,
}

fn main() {}
