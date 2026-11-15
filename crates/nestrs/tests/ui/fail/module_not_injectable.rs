use nestrs::prelude::*;

struct Plain;

#[module(providers = [Plain])]
struct M;

fn main() {}
