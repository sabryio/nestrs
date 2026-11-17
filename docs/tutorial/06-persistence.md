# 6. Persistence

`nestrs-sea-orm` provides the connection, migrations, a readiness check and a shutdown hook as one module, so the blog's
persistence module declares only what is its own business:

<!-- source: examples/blog-api/src/composition/persistence_module.rs -->
```rust
/// Binds the repository ports to adapters chosen by `AppConfig::storage`.
///
/// Importers get the two ports and the [`Transactional`] interceptor; the concrete adapters
/// stay private. The connection, its migrations, its readiness check and its shutdown come
/// from `nestrs_sea_orm::SeaOrmModule`, so nothing here knows how a pool is managed.
#[module(
    imports = [SeaOrmModule::with_migrator::<Migrator>(database_config())],
    providers = [user_repository, post_repository, post_announcements, SeaOutboxStore],
    exports = [UserRepositoryPort, PostRepositoryPort, PostAnnouncementsPort, OutboxStorePort, Transactional],
)]
pub struct PersistenceModule;
```

* `SeaOrmModule::with_migrator::<Migrator>(..)` connects, applies pending migrations at start-up (turn that off with
  `migrate_on_start(false)` when migrations are a separate deploy step), reports readiness on `/health/ready`, and closes
  the pool on shutdown.
* `Transactional` is exported so controllers can ask for it.

## A transaction per request, declared where it matters

Writes in the posts controller carry `#[interceptor(Transactional)]`. The interceptor opens a transaction lazily on first
use and **commits on 2xx/3xx, rolls back on 4xx/5xx**. Repositories get their connection from one place:

<!-- source: examples/blog-api/src/infrastructure/persistence/sea_orm/mod.rs -->
```rust
/// Where to run statements: the request's transaction when the route has one, the pool
/// otherwise. Repositories never need to know which.
async fn executor(database: &Database) -> Result<Executor, RepositoryError> {
    database.executor().await.map_err(unavailable)
}
```

Repositories never begin or commit anything themselves, so a use case that stores a post *and* records an announcement
(chapter 7) is atomic without either repository knowing about the other.

## Adapters translate errors

<!-- source: examples/blog-api/src/infrastructure/persistence/sea_orm/mod.rs -->
```rust
/// Maps a driver error onto the port's error type, so no SeaORM type leaks inward.
fn unavailable(error: impl std::fmt::Display) -> RepositoryError {
    RepositoryError::Unavailable(error.to_string())
}
```

No SeaORM type leaks inward; the domain only ever sees `RepositoryError`.

## Migrations are code you can test

Migrations are ordinary types implementing SeaORM's `MigrationTrait`. The blog's integration test applies them to SQLite
(and, when `BLOG_TEST_POSTGRES_URL` is set, to Postgres), checks they are recorded, and rolls them back:
`cargo test -p blog-api --test migrations`.

**SOLID here.** The SeaORM and in-memory repositories honour the same contract, and the tests run one set of expectations
against both (*Liskov*). Transactions are an interceptor, not code inside use cases (*single responsibility*,
*open/closed*).
