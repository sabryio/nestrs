use nestrs::prelude::*;

struct Settings;

#[provider]
fn banner(settings: Settings) -> String {
    let _ = settings;
    String::new()
}

fn main() {}
