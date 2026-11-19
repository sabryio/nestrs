#[fx::port]
trait Repo<T> {
    fn get(&self) -> T;
}

fn main() {}
