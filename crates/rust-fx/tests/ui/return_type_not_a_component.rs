use std::sync::Arc;

struct Config;

fn config() -> Config {
    Config
}

fn main() {
    // `Config` is neither an `Arc<T>` nor a `Component`.
    let _ = fx::App::new().constructor(config);
    let _: Option<Arc<Config>> = None;
}
