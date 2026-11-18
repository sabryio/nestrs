struct Db;

#[fx::component]
impl Db {
    #[fx::provide]
    fn connect(&self) -> Self {
        Db
    }
}

fn main() {}
