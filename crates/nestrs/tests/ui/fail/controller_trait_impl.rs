use nestrs::prelude::*;

#[injectable]
struct C;

#[controller("/c")]
impl Default for C {
    fn default() -> Self {
        C
    }
}

fn main() {}
