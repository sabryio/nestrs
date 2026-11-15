use nestrs::prelude::*;

struct Plain;

#[module(imports = [Plain])]
struct M;

fn main() {}
