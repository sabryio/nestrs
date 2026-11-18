struct Cache<T>(T);

#[fx::component]
impl<T: Send + Sync + 'static> Cache<T> {
    #[fx::provide]
    fn new() -> Arc<Self> {
        unimplemented!()
    }
}

fn main() {}
