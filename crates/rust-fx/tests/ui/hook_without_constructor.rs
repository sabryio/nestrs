struct Db;

#[fx::component]
impl Db {
    #[fx::on_start]
    async fn warm(&self) {}
}

fn main() {}
