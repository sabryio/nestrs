//! Repository adapters: in-memory (demos, tests) and SeaORM (SQLite or Postgres).

mod memory_posts;
mod memory_users;
mod sea_orm;

pub use memory_posts::InMemoryPostRepository;
pub use memory_users::InMemoryUserRepository;
pub use sea_orm::{Database, DatabaseIndicator, Migrator, SeaPostRepository, SeaUserRepository};
