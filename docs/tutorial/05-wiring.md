# 5. Wiring

Modules are where decisions are made. A module lists what it provides, what it imports, and what it exports.

## A feature module

<!-- source: examples/blog-api/src/composition/posts_module.rs -->
```rust
/// The posts use cases and their controller. Publishing needs the user repository
/// *port* (to check authors), which `PersistenceModule` exports.
#[module(
    imports     = [PersistenceModule, AuthModule, KitModule, SharedStateModule],
    providers   = [BroadcastPostEvents, post_events, PublishPost, GetPost, ListPosts, DeletePost, WatchPosts],
    controllers = [PostsController],
    exports     = [PublishPost],
)]
pub struct PostsModule;
```

Providers are the use cases; the controller is mounted; only `PublishPost` is exported because another module (seeding)
needs it. Nothing here mentions a database.

## Choosing adapters

The one place that knows *which* adapter backs a port is a provider function:

<!-- source: examples/blog-api/src/composition/persistence_module.rs -->
```rust
/// Picks the post repository adapter from `AppConfig::storage`.
#[provider]
fn post_repository(config: Arc<AppConfig>, database: Arc<Database>) -> PostRepositoryPort {
    match config.storage {
        Storage::Memory => Arc::new(InMemoryPostRepository::default()),
        Storage::Database => Arc::new(SeaPostRepository::new(database)),
    }
}
```

Set `BLOG_STORAGE=database` and the SeaORM adapter replaces the in-memory one. No use case, controller or domain test changes.

## Typed configuration

<!-- source: examples/blog-api/src/infrastructure/config.rs -->
```rust
/// Which persistence adapters the application uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Storage {
    /// Process memory; data is lost on restart. Good for demos and tests.
    Memory,
    /// A SQL database through SeaORM (`database_url`).
    Database,
}
```

Configuration is a struct with defaults, layered from `blog-api.toml` and `BLOG_*` environment variables and validated at
start-up, so a bad value stops the process with a message instead of failing later.

## The root

<!-- source: examples/blog-api/src/composition/app.rs -->
```rust
#[module(
    imports     = [
        ConfigModule::for_root::<AppConfig>(ConfigSource::new().file("blog-api.toml").env_prefix("BLOG")),
        HttpModule::module(),
        KitModule,
        HealthModule,
        PersistenceModule,
        MessagingModule,
        outbox_relay(),
        SharedStateModule,
        UsersModule,
        PostsModule,
        SeedModule,
    ],
)]
pub struct AppModule;
```

`AppModule` is a list. Adding a feature means adding one line; `purlin generate feature|module|port` edits this list for you
by parsing it, so comments and formatting survive.

**SOLID here.** Switching `Storage::Memory` for `Storage::Database` edits one provider (*open/closed*, and *Liskov*: both
adapters satisfy the port). Modules export ports, not implementations (*dependency inversion*), and each module has one
concern (*single responsibility*).
