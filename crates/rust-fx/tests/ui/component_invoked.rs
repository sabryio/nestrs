struct Worker;

#[fx::component]
impl Worker {
    #[fx::constructor]
    fn new() -> Self {
        Worker
    }
}

#[fx::module(invokes = [Worker])]
struct Bad;

fn main() {}
