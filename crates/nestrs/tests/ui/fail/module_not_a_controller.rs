use nestrs::prelude::*;

#[injectable]
struct Plain;

#[module(controllers = [Plain])]
struct M;

fn main() {}
