struct Db;

fn uses(_: String) {}

fn main() {
    // `String` is not an injectable parameter: take `Arc<String>` instead.
    let _ = fx::App::new().invoke(uses);
    let _ = Db;
}
