use purlin::prelude::*;

#[injectable]
struct Service {
    #[inject(lazy)]
    inner: Arc<u8>,
}

fn main() {}
