# Changelog

All notable changes are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the crates follow
[Semantic Versioning](https://semver.org/). While the version is `0.x`, a minor bump may
contain breaking changes; they are always listed under **Changed** or **Removed**.

All crates in the workspace share one version and are released together.

## [Unreleased]

### Changed
- **Renamed the project from `nestrs` to `purlin`.** Every crate is now `purlin*` (`purlin`, `purlin-core`,
  `purlin-http`, `purlin-cli`, ...), the binary is `purlin`, Rust paths are `purlin_*`, and the outbox table is
  `purlin_outbox`. The headless DI/runtime layer ships separately as `rust-fx` (library `fx`).
- Minimum supported Rust version is **1.89** (it was declared as 1.85, which the dependency
  tree never actually built with).
- The `purlin` facade has an `http` feature (default on). Without it there is no `axum`, `hyper` or
  `tower`: `purlin-jobs`, `purlin-cache` and `purlin-outbox` are headless, and `purlin-health` and
  `purlin-sea-orm` are headless with `default-features = false` (the health routes and the per-request
  `Transactional` interceptor are behind their own `http` feature). `purlin` re-exports `rust-fx` as `purlin::fx`.
  `scripts/check-headless.sh` enforces it in CI.

### Added
- `purlin::hosting`: `hosted::<AppModule>()` runs a purlin module graph inside an `fx::App` (bootstrap at start, destroy hooks at stop, `Arc<Hosted>` for other fx components); works without the `http` feature.
- `rust-fx` (library `fx`): a headless, fx-style application framework with no HTTP dependency.
  Function constructors (`provide`), `invoke`, `decorate`, `supply`, `bind!` (fx.As), named
  values and groups, `Option<Arc<T>>`, modules with private scope and exports, lifecycle hooks
  with rollback, `Shutdowner`, start/stop timeouts, `validate()`, observers, `App::get`, and
  `fx::test` (feature `test`) with `replace`.
- `rust-fx-macros`: `#[fx::component]`, `#[fx::module]`, `#[derive(fx::Params)]`,
  `#[derive(fx::Component)]`.
- `rust-fx/examples/worker.rs`: a worker with no web stack.
- **`purlin-sea-orm`**: managed SeaORM connection with lifecycle hooks, migrations behind a port,
  health indicator, `Executor`, per-request `Transactional` interceptor, `UnitOfWork` and
  `Database::in_transaction`. The blog example uses it instead of its own wrapper.
- **`purlin-outbox`**: transactional outbox. `Outbox` records events through the request's
  transaction; `OutboxRelay` delivers at least once with per-key ordering, exponential back-off, dead
  letters and lease-based claims; `SeaOutboxStore`, `MemoryOutboxStore`, `QueuePublisher`,
  `CreateOutbox`, `OutboxModule`. The blog announces published posts through it.
- **`purlin-otel`**: OpenTelemetry adapter. `Telemetry` (OTLP/HTTP export, parent-based ratio
  sampling), `HttpTracing` server-span interceptor, `OtelPropagation` for W3C trace context through
  queues. `purlin-jobs` now emits `job <name>` and `<queue> process` spans and carries an optional
  `traceparent` in queue envelopes (`TracePropagation` port, `JobsConfig::propagation`);
  `purlin-outbox` emits `outbox.publish`. `JobsModule::in_memory_with` and `Service::without_logging`
  support custom configuration. The blog exports traces when `BLOG_OTLP_ENDPOINT` is set.
- **`purlin-cli`**: `generate contract` (TypeScript oRPC contract from the Rust controllers and DTOs, via
  `syn`; `--check`, `--strict`, `--also`), `generate use-case|controller|port`, `doctor` (layer-rule and
  unregistered-controller checks), and `--dry-run` now prints a unified diff computed on an in-memory overlay.
  `Workspace` gained `rust_files`. `Service::without_logging`. The blog's demo seeder moved from
  `infrastructure` to `composition` (found by `doctor`); `examples/react-orpc` generates its contract with the CLI.
- **Tutorial** (`docs/tutorial/`): nine chapters building the blog, with a test that fails when a quoted block drifts
  from `examples/blog-api`. CI runs `purlin doctor` and `generate contract --check --strict` on the blog.
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
- **CLI**: `purlin new` and `purlin generate feature|module|job|queue-handler`, editing existing
  Rust files by parsing them.
- **Quality**: benchmarks against plain axum and a full-stack soak test.
