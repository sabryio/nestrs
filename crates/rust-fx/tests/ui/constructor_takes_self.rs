struct Db;

#[fx::component]
impl Db {
    #[fx::constructor]
    fn connect(&self) -> Self {
        Db
    }
}

fn main() {}
