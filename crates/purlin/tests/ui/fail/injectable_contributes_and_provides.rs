use purlin::prelude::*;

trait Port: Send + Sync {}

#[injectable(provides = Arc<dyn Port>, contributes = Arc<dyn Port>)]
struct Both;
impl Port for Both {}

fn main() {}
