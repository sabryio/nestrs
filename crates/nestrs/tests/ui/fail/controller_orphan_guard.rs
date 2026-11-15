use nestrs::prelude::*;

#[injectable]
struct G;

impl Guard for G {
    async fn check(&self, _parts: &mut Parts) -> Result<(), HttpError> {
        Ok(())
    }
}

#[injectable]
struct C;

#[controller("/c")]
impl C {
    #[guard(G)]
    async fn helper(&self) {}

    #[get]
    async fn handler(&self) {}
}

fn main() {}
