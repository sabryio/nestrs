# purlin-sea-orm

SeaORM for purlin: a managed connection, migrations, per-request transactions and a health indicator.

- `Database`: the pool as a provider. Connects, applies migrations on start, closes at shutdown.
- `Executor`: the pool or the request's transaction; implements SeaORM's `ConnectionTrait`, so
  repositories do not know which they are using.
- `Transactional`: interceptor that runs a request in one transaction. Commits on success, rolls back
  on `4xx`/`5xx`.
- `UnitOfWork` and `Database::in_transaction`: explicit transactions for jobs and scripts.
- `Migrations` port with `SeaMigrator<M>` for a `sea-orm-migration` migrator.
- `DatabaseIndicator`: readiness check named `database`.
- `SeaOrmModule`: wires it all.

```toml
[dependencies]
purlin-sea-orm = { version = "0.1", features = ["postgres"] }  # or "sqlite" / "mysql"
```

Part of [purlin](https://github.com/sabry-awad97/purlin). Licensed under MIT or Apache-2.0, at your option.
