use nestrs::prelude::*;

#[port]
trait Repository<T> {
    fn all(&self) -> Vec<T>;
}

fn main() {}
