//! Schema migrations, applied in order and recorded in the `seaql_migrations`
//! table, so each runs exactly once per database.
//!
//! To change the schema, add a new `mNNNN_description.rs` file, register it below
//! and never edit one that has shipped. Names sort by their timestamp prefix.

use sea_orm_migration::prelude::*;

mod m20261110_000001_create_users;
mod m20261110_000002_create_posts;

/// Every migration this application knows, oldest first.
pub struct Migrator;

impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20261110_000001_create_users::Migration),
            Box::new(m20261110_000002_create_posts::Migration),
        ]
    }
}
