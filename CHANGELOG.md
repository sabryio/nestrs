# Changelog

All notable changes are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the crates follow
[Semantic Versioning](https://semver.org/). While the version is `0.x`, a minor bump may
contain breaking changes; they are always listed under **Changed** or **Removed**.

All crates in the workspace share one version and are released together.

## [Unreleased]

### Changed
- Minimum supported Rust version is **1.89** (it was declared as 1.85, which the dependency
  tree never actually built with).

### Added
- **`nestrs-sea-orm`**: managed SeaORM connection with lifecycle hooks, migrations behind a port,
  health indicator, `Executor`, per-request `Transactional` interceptor, `UnitOfWork` and
  `Database::in_transaction`. The blog example uses it instead of its own wrapper.
- **`nestrs-outbox`**: transactional outbox. `Outbox` records events through the request's
  transaction; `OutboxRelay` delivers at least once with per-key ordering, exponential back-off, dead
  letters and lease-based claims; `SeaOutboxStore`, `MemoryOutboxStore`, `QueuePublisher`,
  `CreateOutbox`, `OutboxModule`. The blog announces published posts through it.
- Crate metadata (keywords, categories, homepage, readme), license files and a README for every
  published crate.
- `RELEASING.md`, a release workflow that publishes in dependency order, and CI jobs for MSRV,
  rustdoc warnings and semver compatibility.

## [0.1.0] - unreleased

First feature-complete version.

### Added
- **Core**: modules, dependency injection by type or `dyn Trait` (ports), request scopes,
  multi-binding, lifecycle hooks, test overrides.
- **HTTP**: `#[controller]` routes, guards, role and permission requirements, interceptors,
  pipes, exception filters, validation, OpenAPI generation, `Mount` for hand-written routers,
  Server-Sent Events with resume, WebSockets, graceful shutdown.
- **Interceptors**: circuit breaker, bulkhead, retry, timeout, rate limit, response cache,
  idempotency keys, request ids, metrics, trace context, audit, ETag and more.
- **Jobs**: scheduled jobs (interval and cron) with distributed locks; queues with retries,
  back-off, dead letters and lease-based at-least-once delivery.
- **Cache**: single-flight read-through service cache.
- **Redis**: rate limiting, response cache, idempotency, job locks, queue backend, value store,
  event bus, health indicator.
- **CLI**: `nestrs new` and `nestrs generate feature|module|job|queue-handler`, editing existing
  Rust files by parsing them.
- **Quality**: benchmarks against plain axum and a full-stack soak test.
