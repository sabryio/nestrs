trait Clock: Send + Sync {}
struct System;
impl Clock for System {} // forgot #[fx::bind]

fn main() {
    let _ = fx::bind::<dyn Clock, System>();
}
