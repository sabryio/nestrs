struct Cache<T>(T);

#[fx::component]
impl<T: Send + Sync + 'static> Cache<T> {
    #[fx::constructor]
    fn new() -> Arc<Self> {
        unimplemented!()
    }
}

fn main() {}
