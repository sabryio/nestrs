trait Clock: Send + Sync {}
struct Wrapper<T>(T);

#[fx::bind]
impl<T: Send + Sync + 'static> Clock for Wrapper<T> {}

fn main() {}
