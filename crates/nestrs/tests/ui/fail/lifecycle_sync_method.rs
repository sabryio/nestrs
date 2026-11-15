use nestrs::prelude::*;

struct Pool;

#[lifecycle]
impl Pool {
    #[on_init]
    fn connect(&self) -> Result<(), String> {
        Ok(())
    }
}

fn main() {}
