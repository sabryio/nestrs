trait Clock: Send + Sync {}
struct System;
impl Clock for System {}

#[fx::component]
impl System {
    #[fx::constructor(as = dyn Clock)]
    fn new() -> Self {
        Self
    }
}

fn main() {}
