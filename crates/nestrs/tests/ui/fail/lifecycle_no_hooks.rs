use nestrs::prelude::*;

struct Pool;

#[lifecycle]
impl Pool {
    async fn connect(&self) -> Result<(), String> {
        Ok(())
    }
}

fn main() {}
