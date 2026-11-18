use purlin::prelude::*;

use super::seed::DemoDataSeeder;
use super::{PostsModule, UsersModule};

/// Seeds demo data through the real use cases, so it imports the modules that
/// export them. The seeder's `#[on_init]` hook runs after everything is built.
#[module(
    imports   = [UsersModule, PostsModule],
    providers = [DemoDataSeeder],
)]
pub struct SeedModule;
