struct Worker;

#[fx::component]
impl Worker {
    #[fx::provide]
    fn new() -> Self {
        Worker
    }
}

#[fx::module(invokes = [Worker])]
struct Bad;

fn main() {}
