# nestrs

NestJS-style architecture for Rust on top of Axum: **modules** that declare what they
provide and export, **dependency injection** keyed by type or port (a trait), and
**controllers** whose routes are plain `async fn` methods.

There is no runtime reflection. Attribute macros read your declarations and emit ordinary
trait impls; the module graph is then validated **at startup** (before any provider is
built), so wiring mistakes are reported as precise errors rather than surfacing mid-request.

```rust
use nestrs::prelude::*;

#[port]                                         // a trait the service depends on
pub trait UserRepository {
    async fn find(&self, id: Uuid) -> Result<Option<User>, RepositoryError>;
}                                               // generates `UserRepositoryPort`, no `dyn` to write

#[injectable]
pub struct UsersService {
    repository: UserRepositoryPort,             // depends on an abstraction
}

#[injectable]
pub struct UsersController {
    service: Arc<UsersService>,
}

#[controller("/users")]
impl UsersController {
    #[get("/{id}")]
    async fn find(&self, Path(id): Path<Uuid>) -> Result<Json<User>, HttpError> {
        Ok(Json(self.service.find(id).await?))
    }

    #[post]
    #[guard(AuthGuard)]
    #[status(201)]                                       // default success status, applied and documented
    #[response(409, "The email is already registered")]  // extra documented error
    async fn create(&self, ValidatedJson(dto): ValidatedJson<CreateUserDto>)
        -> Result<Json<User>, HttpError>
    {
        Ok(Json(self.service.create(dto).await?))
    }
}

#[module(
    imports     = [AuthModule],
    providers   = [PgUserRepository, UsersService],   // PgUserRepository: #[injectable(provides = UserRepositoryPort)]
    controllers = [UsersController],
    exports     = [UsersService],
)]
pub struct UsersModule;

#[tokio::main]
async fn main() -> Result<(), AppError> {
    App::create(AppModule).await?
        .global_prefix("/api")
        .layer(TraceLayer::new_for_http())
        .listen("0.0.0.0:3000")
        .await
}
```

## Smallest example

`examples/todo-minimal/src/main.rs` is a complete app in one file (port, adapter, controller, module,
tests, no layering): `cargo run -p todo-minimal`. `examples/blog-api` is the same ideas organised as clean
architecture with a database, config, OpenAPI and a fuller test suite.

The workspace uses Rust **edition 2024** (MSRV 1.89) and **syn 3** for the macros.

## NestJS → nestrs

| NestJS | nestrs |
|---|---|
| `@Module({ imports, providers, controllers, exports })` | `#[module(imports=[..], providers=[..], controllers=[..], exports=[..])]` |
| `@Injectable()` | `#[injectable]` (fields are `Arc<T>` or a port alias, or `#[inject(default)]`) |
| `{ provide: Token, useClass: Impl }` | `#[injectable(provides = TraitPort)]` on `Impl` (or `bind!(dyn Trait => Impl)`) |
| `useValue` / `useFactory` (async) | `Provider::value(..)` / `#[provider] async fn make(dep: Arc<A>) -> Result<T, DiError>` |
| `@Controller('/x')`, `@Get(':id')` | `#[controller("/x")]`, `#[get("/{id}")]` |
| `@UseGuards(AuthGuard)` | `#[guard(AuthGuard)]` on a route or the whole `impl` |
| Pipes | `Piped<MyPipe>` extractor over an injectable `Pipe`; `ValidatedJson<T>` for `validator` rules |
| Exception filters | injectable `ExceptionFilter`, `App::builder(..).filter::<F>()` |
| Interceptors | `#[interceptor(MyInterceptor)]` (injectable) or a value, `#[interceptor(RateLimit::per_minute(60))]`; app-wide via `App::interceptor`; plain middleware via `App::layer` |
| Authentication | `BearerAuth` guard + `Authenticator` port (`AuthError`), `StaticTokens` adapter; 401 with `WWW-Authenticate`, 503 if the verifier is down |
| Authorization | `Principal` + `#[roles(..)]` (any of) / `#[permissions(..)]` (all of); 401 vs 403; documented in OpenAPI |
| Rate limits, deadlines | `RateLimit` (GCRA, `RateLimitStore` port, `ratelimit-*` + `Retry-After`, 429) and `Timeout` (504); both are ordinary interceptors |
| `Scope.REQUEST` / `Scope.TRANSIENT` | `#[injectable(scope = request)]` / `scope = transient`, resolved with `Inject<T>` |
| `@nestjs/config` | `nestrs::config::ConfigModule::for_root::<T>(ConfigSource::new().file(..).env_prefix(..))` |
| `@nestjs/swagger` | `#[derive(ToSchema)]` on DTOs, `App::serve_openapi(ApiInfo::new(..))` |
| `@nestjs/testing` | `nestrs-testing`: `TestApp::builder(AppModule).mock::<TraitPort>(Arc::new(fake))`, `TestClient` |
| `NestFactory.create(AppModule)` | `App::create(AppModule).await?` |
| `Test.createTestingModule(..).overrideProvider(..)` | `App::builder(AppModule).override_provider(..)` |
| `OnModuleInit` / `OnModuleDestroy` | `#[lifecycle] impl T { #[on_init] async fn .. #[on_destroy] async fn .. }` |
| `ConfigModule.forRoot(..)`, `@Global()` | `ConfigModule::for_root(..) -> ModuleRef`, `#[module(global)]` |

## Writing it without `dyn`

Rust has no reflection, so every dependency is declared, but the declarations stay close to plain
Rust. Four attributes cover the abstraction side:

```rust
// 1. A port: a trait the rest of the code depends on.
#[port]                                          // = async_trait + Send + Sync + the alias below
pub trait Mailer { async fn send(&self, to: &str) -> Result<(), MailError>; }
// generated: pub type MailerPort = Arc<dyn Mailer>;     (#[port(Mail)] picks another alias name)

// 2. An adapter: provides the port. Fields are injected by type.
#[injectable(provides = MailerPort)]
pub struct SmtpMailer { config: Arc<AppConfig> }

#[async_trait]
impl Mailer for SmtpMailer { async fn send(&self, to: &str) -> Result<(), MailError> { .. } }

// 3. A consumer: no `new`, no factory, no `dyn`.
#[injectable]
pub struct Welcome { mailer: MailerPort, users: UserRepositoryPort }

// 4. Choosing at start-up (NestJS `useFactory`): parameters are dependencies.
#[provider]
fn mailer(config: Arc<AppConfig>) -> MailerPort {          // sync or async, plain value, Arc,
    if config.smtp { Arc::new(SmtpMailer::new(..)) }        // port alias, or a Result of those
    else           { Arc::new(NoopMailer) }
}

#[module(providers = [Welcome, mailer], exports = [MailerPort, Welcome])]
pub struct MailModule;
```

How it works: a field type is a dependency when it implements `Dependency` (`Arc<T>`, so a port alias
qualifies because it *is* `Arc<dyn Trait>`); resolution is driven by the type, not by how it is spelled.
`exports` and `providers` accept the alias, a type, or `dyn Trait`. Lifecycle hooks are found by the
compiler: if a type implements `OnModuleInit` / `OnModuleDestroy` (write them with `#[lifecycle]`), they are
attached to its provider with no extra declaration, including for a `provides` adapter and for the value a
`#[provider]` function returns. `#[injectable(scope = request)]` and `scope = transient` replace
`Provider::of::<T>().request()`.

In tests: `.mock::<MailerPort>(Arc::new(FakeMailer::default()))`. Keep a clone of the `Arc` to assert on.

The explicit API (`Provider::..`, `bind!`, `factory_arc`) is still there and expands to the same thing; use it
when the attributes do not fit (hand-written `Injectable`, dynamic provider lists).

## What is checked at startup

Before anything is constructed, `bootstrap` verifies the whole graph and fails with a
`DiError` naming the modules and types involved:

| Mistake | Error |
|---|---|
| depends on something nobody provides | `MissingProvider` |
| depends on something a module provides but did not export (or you did not import) | `NotVisible` |
| two modules provide the same type | `DuplicateProvider` |
| a module exports what it neither provides nor re-exports | `InvalidExport` |
| modules import each other in a loop | `ModuleCycle` (`A -> B -> A`) |
| providers depend on each other in a loop | `CircularDependency` |
| a factory fails | `Factory` (attributed to the provider) |
| a singleton depends on a request-scoped or transient provider | `ScopeMismatch` |
| an init or destroy hook fails | `Hook` (provider and phase) |
| an override targets a type nobody provides | `UnknownOverride` |

Guards are checked like any other dependency, even though they are resolved when routes are
mounted rather than when the controller is built.

## Testing and composition

**Provider overrides.** Swap any provider for a test without touching the modules:

```rust
let app = App::builder(AppModule)
    .override_provider(Provider::instance::<dyn UserRepository>(Arc::new(SeededRepository::new())))
    .override_provider(Provider::value(AppConfig { api_token: "test".into(), ..Default::default() }))
    .build()
    .await?;
```

Overrides replace the provider for *every* dependent in every module. The graph is still
validated: an override nobody provides is `UnknownOverride` (usually a typo or a stale test),
and an overriding provider's own dependencies must exist. An overridden provider is exempt from
visibility checks, because a test double has no module of its own. The last override of a type wins.
`Overrides` is a plain value, so fixtures can be built once and applied with `.overrides(..)`.

**Configurable modules.** A module can be a value, not just a type (`ConfigModule.forRoot()`):

```rust
impl ConfigModule {
    pub fn for_root(config: AppConfig) -> ModuleRef {
        ModuleDef::new("ConfigModule").global()
            .providers([Provider::value(config)]).exports([Token::of::<AppConfig>()])
            .into_ref()
    }
}

#[module(imports = [ConfigModule::for_root(AppConfig::from_env()), UsersModule])]
struct AppModule;
```

In `imports` and `providers`, a bare path means a type; any other expression is a ready-made
`ModuleRef` or `Provider`. Each `into_ref()` is a distinct module instance; clone the `ModuleRef`
to import the same instance from several places. Two *separate* instances providing the same type
are reported as `DuplicateProvider`.

**Global modules.** `.global()` or `#[module(global)]` makes a module's exports visible everywhere
without importing it. Use it sparingly (config, logging); everything else should stay explicit.

**Compile-fail tests.** `crates/nestrs/tests/ui/` holds `trybuild` cases that pin the macros'
diagnostics: `pass/` must compile, `fail/*.rs` must not, and each `.stderr` snapshot is the exact
message users see (non-struct `#[injectable]`, non-injectable fields, unknown `#[module]` keys, imports
that are not modules, controllers without `&self`, a bad guard, a bad extractor, and so on).
Handler parameters are checked individually, so a wrong extractor is reported on that parameter.
After changing a macro, review diffs with `TRYBUILD=overwrite cargo test -p nestrs --test ui`.

## Lifecycle hooks

Mark the methods with `#[lifecycle]`; the hooks attach themselves to the type's provider:

```rust
#[lifecycle]
impl Pool {
    #[on_init]    async fn connect(&self) -> Result<(), sqlx::Error> { .. }
    #[on_destroy] async fn close(&self)   -> Result<(), sqlx::Error> { .. }   // error: any `Display`
}

#[module(providers = [Pool, Migrator])]       // hooks come with the types
```

The attribute implements `OnModuleInit` / `OnModuleDestroy` for you. By hand, implement the traits and
attach them explicitly:

```rust
providers = [
    Provider::of::<Pool>().with_destroy::<Pool>(),
    Provider::of::<Migrator>().with_init::<Migrator>(),
    bind!(dyn Mailer => Smtp).on_init::<dyn Mailer>(|mailer| Box::pin(async move { mailer.verify().await })),
]
```

- **Init** runs after the whole graph is built, dependencies first, before controllers are mounted.
  If a hook fails, startup aborts with `DiError::Hook`, and the providers that had already started are
  destroyed.
- **Destroy** runs in the reverse order (a service goes before what it depends on). `App::listen` runs
  it after the server drains; in tests call `app.shutdown().await`. A failing hook does not stop the
  others: all errors are returned. Running shutdown twice is a no-op.
- Hooks belong to the provider, so an override replaces them with the fake (which has none).

## Request scopes and transient providers

Providers are singletons by default. Two shorter lifetimes exist for per-request state:

```rust
#[injectable]
struct RequestContext { info: Arc<RequestInfo> }          // what request am I serving?

#[module(
    imports   = [HttpModule::module()],                    // declares RequestInfo (once, in the root)
    providers = [Provider::of::<RequestContext>().request(), Provider::of::<Ticket>().transient()],
)]
struct AppModule;

#[get]
async fn show(&self, Inject(ctx): Inject<RequestContext>) -> .. { .. }
```

- **Request**: built on first use, at most once per request. **Transient**: built on every resolution.
- `#[injectable]` types work unchanged in any lifetime: a scoped provider's dependencies are resolved
  (async, recursively) into a small overlay container and its ordinary `build` runs against it.
- **The lifetime rule, checked at startup:** only a non-singleton may depend on a non-singleton.
  A singleton holding a request-scoped value would silently pin the first request's state forever, so
  the graph fails with `ScopeMismatch`. Controllers are singletons: they take scoped values as
  `Inject<T>` *parameters*, never as fields.
- `#[controller]` notices `Inject<T>` / `Piped<P>` parameters and checks that `T` / `P` exist and are
  visible to the controller's module, so a missing provider is a startup error, not a 500.
- `RequestInfo` (method, URI, headers) is seeded into every scope by `HttpModule`.
- Lifecycle hooks run for singletons only.

## Shared state: fields and `Inject<T>` (there is no axum `State`)

axum's `State` is one value fixed when the router is built. nestrs replaces it with the container: state is a
provider, modules declare and export it, tests override it, and the graph is checked at startup.

```rust
#[injectable]
struct Counter { hits: AtomicU64 }                       // shared state = a singleton provider

#[injectable]
struct Api { counter: Arc<Counter>, store: TodoStorePort }   // 1. a field: every route uses it

#[controller("/x")]
impl Api {
    #[get]
    async fn hits(&self) -> String { self.counter.hits.load(Ordering::Relaxed).to_string() }

    #[get("/summary")]                                    // 2. Inject<T>: this handler needs one more thing
    async fn summary(&self, Inject(audit): Inject<AuditLog>) -> .. { .. }
}
```

| Use | When |
|---|---|
| a controller **field** | the service is used by most routes (the default) |
| **`Inject<T>`** parameter | one handler needs an extra service, or a **request-scoped / transient** provider (controllers are singletons and cannot hold those as fields) |

`Inject<T>` takes ports too (`Inject<dyn TodoStore>`) and is validated at startup like a field. Using
`axum::extract::State` in a controller is a compile error that says so. For `Path`, `Query`, `Json`, headers and the
other axum extractors nothing changes.

**Raw axum, when you need it.** `App::builder(AppModule).merge(router)` mounts a hand-written axum `Router` (a webhook,
a WebSocket upgrade, a third-party router, real axum `State` kept inside it with `.with_state(..)`) next to the
controllers. Merged routes share request scopes (their handlers can take `Inject<T>`), exception filters,
`global_prefix`, `App::interceptor` and `App::layer`; they do not get `#[guard]`/`#[roles]`/route-level interceptors
or OpenAPI docs, and a path that collides with a controller route panics at build, as in axum. `TestApp::builder`
has the same `.merge`. `examples/todo-minimal` shows both styles (`SummaryController` has no fields).

## Pipes, interceptors and exception filters

Request pipeline, outside in: **filters -> guards -> interceptors -> pipes/extractors -> handler**.

```rust
// Pipe: transform or reject one input. Injectable, so it can use services.
#[injectable] struct PaginationPipe;   // shipped by nestrs-kit
impl Pipe for PaginationPipe {
    type Input = Query<PageQuery>;
    type Output = Pagination;
    async fn transform(&self, Query(q): Query<PageQuery>) -> Result<Pagination, HttpError> { .. }
}
#[get] async fn list(&self, Piped(p): Piped<PaginationPipe>) -> .. { .. }

// Interceptor: code around the handler. First declared is outermost.
impl Interceptor for Timing {
    async fn intercept(&self, request: Request, next: Next) -> Response { .. next.run(request).await .. }
}
#[controller("/users")] #[interceptor(Timing)] impl UsersController { .. }

// Exception filter: sees every HttpError on its way out; may replace the response or just observe.
impl ExceptionFilter for ErrorLog {
    async fn catch(&self, error: &HttpError, request: &RequestInfo) -> Option<Response> { .. None }
}
App::builder(AppModule).filter::<ErrorLog>().build().await?
```

### Bearer authentication

```rust
#[injectable(provides = AuthenticatorPort)]          // the port: token -> Principal (static table, JWT, introspection ..)
struct JwtAuthenticator { .. }
impl Authenticator for JwtAuthenticator {
    fn authenticate<'a>(&'a self, token: &'a str) -> BoxFuture<'a, Result<Principal, AuthError>> { .. }
}

#[module(providers = [JwtAuthenticator, BearerAuth], exports = [BearerAuth])]
struct AuthModule;

#[controller("/posts")]
#[guard(BearerAuth)]
impl PostsController { .. }
```

- `BearerAuth` reads `Authorization: Bearer <token>` (scheme case-insensitive, exactly one header) and stores the `Principal` for
  `#[roles]`, `#[permissions]` and the `Principal` extractor.
- Missing, malformed, unknown or expired token: **401** with `WWW-Authenticate: Bearer`; the token is never echoed.
  `AuthError::Unavailable` (key server down) fails **closed** with 503.
- `StaticTokens::new().token("..", Principal::new(..)).provider()` is a ready adapter for development, tests and service keys
  (constant-time comparison).

### Optional authentication, token caching and JWT

```rust
#[get("/me")] #[guard(OptionalBearerAuth)]
async fn me(&self, principal: Option<Principal>) -> .. { .. }   // anonymous allowed; a bad token is still 401

// Wrap any Authenticator so an identity provider is not hit on every request.
let verifier = nestrs_http::cached(Arc::new(MyIdp), CacheConfig::default()); // 30s hits, 5s misses, 10k entries
// Unavailable is never cached, so an outage heals as soon as the provider does.

// nestrs-jwt: a ready-made Authenticator. Algorithms are an explicit allow-list (no default),
// exp and sub are required, issuer/audience are checked when configured.
let config = JwtConfig::rs256().issuer(["https://id.example"]).audience(["blog"]);
JwtAuthenticator::provider(Arc::new(StaticKeys::jwks(JWKS_JSON)?), config)
```

`KeyBy::Caller` rate-limits per authenticated subject (falling back to the IP).

### nestrs-kit

The plumbing every service writes, packaged once: `KitModule` (timing interceptor, request-scoped
`RequestContext`, `PaginationPipe`) and `Service`, which is the whole `main`:

```rust
Service::new("Blog API", env!("CARGO_PKG_VERSION"))
    .prefix("/api").rate_limit(RateLimit::per_second(50).scope("global")).timeout(Duration::from_secs(30))
    .run(AppModule, |c| Ok(c.resolve::<AppConfig>()?.bind_addr.clone())).await
```

### nestrs-interceptors

A catalogue of ready-made interceptors. Each is a plain value (`#[interceptor(..)]` on a route or
controller, or `Service::interceptor(..)` / `App::interceptor(..)` for the whole app). Anything with state
or an outside system sits behind a small port so the in-memory default can be swapped.

| Group | Interceptors | Port |
|---|---|---|
| Resilience | `CircuitBreaker`, `Bulkhead`, `Retry` | |
| Caching | `ResponseCache`, `ETag`, `SingleFlight` | `CacheStore` |
| Safe writes | `Idempotency` (`Idempotency-Key`) | `IdempotencyStore` |
| Observability | `RequestId`, `Metrics` + `PrometheusRegistry`, `TraceContextInterceptor` (W3C), `Audit`, `SlowRequestLog` | `MetricsSink`, `AuditSink` |
| HTTP hygiene | `SecurityHeaders`, `BodyLimit`, `Deprecated` (Sunset) | |
| Request context | `TenantResolver`, `LocaleNegotiator`, `ApiVersions` (extractors: `Tenant`, `Locale`, `ApiVersion`) | |

```rust
#[post]
#[guard(BearerAuth)]
#[interceptor(RateLimit::per_minute(60).by(KeyBy::Caller))]
#[interceptor(Idempotency::in_memory())]          // a retried POST replays the first response
async fn create(..) { .. }

Service::new("API", "1.0")
    .interceptor(Metrics::prometheus(registry).expose_at("/api/metrics"))
    .interceptor(RequestId::default())            // last added = outermost
```

Safety defaults worth knowing: `ResponseCache` and `SingleFlight` never share a response between callers that
send `Authorization` or `Cookie`; `Retry` only repeats idempotent methods; `Idempotency` scopes keys per caller
and rejects a reused key with a different body; `CircuitBreaker` counts consecutive `5xx` (so `Timeout`'s 504
counts) and probes with one request when half-open. CORS and compression are `tower-http` layers, and a
transaction per request belongs behind your own unit-of-work port, so they are not interceptors here.

### nestrs-redis

Redis adapters for the ports above, so several instances share state: `RedisRateLimitStore` (GCRA decided inside
Redis by one Lua script, using the server's clock), `RedisCacheStore` and `RedisIdempotencyStore`. One
`RedisConnection` (cheap to clone, reconnects, 500 ms command timeout) serves all three.

```rust
let redis = RedisConnection::connect("redis://127.0.0.1/").await?;
Idempotency::new(Arc::new(RedisIdempotencyStore::new(redis.clone())))
RateLimit::per_minute(60).store(Arc::new(RedisRateLimitStore::new(redis)))
```

When Redis is down each adapter does what its port promises: the rate limiter errors (and `RateLimit` fails *open*),
the cache misses, idempotency answers `Unavailable` (and the interceptor fails *closed* with `503`, since running a
write unprotected could repeat it). A running request's idempotency claim expires after `pending_ttl` (60 s), so a
crashed instance never locks a client out. The blog example picks the store from `BLOG_REDIS_URL` in
`SharedStateModule`. The integration tests start a real `redis-server` and skip when it is not installed.

### Redis event feed (cross-instance SSE)

`RedisEventStream` is a Redis Stream behind the typed-SSE contract: every instance publishes into one ordered feed
and serves clients from it, and the stream id is the SSE `id:`, so a `Last-Event-ID` taken from one instance resumes
on another with no gap and no duplicate. One reader task per process does `XREAD BLOCK` and fans out locally; a
subscriber takes the live receiver first and then replays history, deduplicating by id. The blog picks it when
`BLOG_REDIS_URL` is set (`post_events` provider) and keeps the in-process broadcast otherwise.

### WebSockets

A WebSocket is a `GET` route that upgrades, so guards, roles, interceptors and scopes all run *before* the upgrade
and a rejected request never becomes a socket. `Ws` + `Channel<In, Out>` add the conversation: JSON text frames
decoded to your types, pings answered, bad frames surfaced as `Err` without closing, and a `1001 Going Away` close
frame sent when the app shuts down.

```rust
#[get("/ws")]
#[guard(BearerAuth)]
async fn connect(&self, ws: Ws, principal: Principal) -> Response {
    ws.on_upgrade(move |mut channel: Channel<ClientMsg, ServerMsg>| async move {
        while let Some(message) = channel.recv().await { /* Ok(msg) | Err(decode error) */ }
    })
}
```

### nestrs-jobs: scheduled tasks and queues

```rust
#[module(imports = [JobsModule::in_memory()], providers = [PurgeDrafts, SendWelcomeEmail])]
struct AppModule;

#[injectable(contributes = JobPort)]            // runs on a schedule, from any module
struct PurgeDrafts { .. }                       // schedule(): Schedule::cron("0 0 * * * *") or ::every(d)

#[injectable(contributes = QueueHandlerPort)]   // consumes a queue, from any module
struct SendWelcomeEmail { .. }                  // handle(&str) -> Result<(), HandlerError::{Retry, Fatal}>

self.queue.enqueue("welcome-emails", &Welcome { user }).await?;   // inject `Queue` to produce
```

Jobs: a `JobLock` makes one instance run each tick, a job never overlaps itself, and a timeout or panic is a logged
failure. Queues are at-least-once with **leases**: a worker reserves work for `JobsConfig::lease`, renews it while the
handler runs (so a long job is never mistaken for a dead worker), and finishes with ack, retry (exponential backoff)
or bury (after `max_attempts`, at once for `Fatal`). If a worker dies, its lease expires and a reaper
(`reap_every`) returns the work to the queue with one more attempt, or buries it at the limit, so work that kills
its worker cannot loop forever and nothing needs a restart. A worker that lost its lease cannot ack, retry or bury
work that moved on. Everything stops within `JobsConfig::shutdown_grace`. `nestrs-redis` supplies `RedisJobLock` and
`RedisQueueBackend` (waiting list + sorted set of leases scored by server time, atomic Lua for reserve, renew,
retry, bury and reap; `reserve` polls every 50 ms while empty). `JobsModule::with_backends(config, lock, queue)`
plugs them in.

### nestrs-cache: service-level caching

`ServiceCache` is a read-through cache for use cases and repositories (below the HTTP `ResponseCache`), so HTTP, jobs
and queue handlers all benefit: `cache.get_or_load(key, || loader).await`, `cache.invalidate(key)`. One load per key
per process (single flight), a failing store is a miss (fail open), loader errors are never cached.
`MemoryValueStore` (bounded) and `RedisValueStore` implement the `ValueStore` port. Use it as a decorator over a
repository port so callers do not change.

### The `nestrs` CLI

```text
cargo install --path crates/nestrs-cli      # installs `nestrs`
nestrs new shop-api                         # clean-architecture service that boots and has a passing test
cd shop-api && nestrs generate feature invoice
```

`generate module|job|queue-handler <name>` add an empty feature module, a scheduled job, or a queue consumer (with its
message type); jobs and handlers also add `nestrs-jobs` to `Cargo.toml` (following however `nestrs` is declared),
import `JobsModule::in_memory()` once, and register themselves as providers. `AppModule` is edited by parsing it
with `syn` (spans give exact offsets) and splicing, so multi-line lists, comments and trailing commas survive;
new files are run through `rustfmt`, existing ones are only spliced. `generate feature` writes one slice through every layer (domain entity + repository port, use cases, in-memory
adapter, controller + DTOs + error mapping, module, an HTTP test) and registers it in each layer's `mod.rs` and in
`AppModule`. It never overwrites (`--force` to), is idempotent, supports `--dry-run`, and reports anything it could
not patch so you can do it by hand. `nestrs new --nestrs-path <checkout>` points the project at a local nestrs.
`cargo test -p nestrs-cli -- --ignored` generates a project with two features and checks it builds, passes its tests
and is clippy-clean.

### Benchmarks and soak (`nestrs-bench`)

`cargo bench -p nestrs-bench` measures `GET /hello` in process (no sockets) as plain axum, then nestrs with one more
feature per row. On the development machine (noisy, shared; compare neighbours, not absolutes):

| Variant | time per request |
|---|---|
| axum (baseline) | 0.72 µs |
| axum + 3 pass-through middlewares | 1.77 µs |
| nestrs controller | 2.43 µs |
| + `Inject<T>` argument | 2.71 µs |
| + bearer guard | 3.13 µs |
| + 3 interceptors | 5.83 µs |
| guard + role + 3 interceptors + rate limit | 10.1 µs |

A nestrs app has a fixed cost of roughly 1.7 µs over bare axum, about two thirds of which is what three pass-through
axum middlewares cost (request scopes, filters, shutdown signal); each interceptor adds about 1 µs. Against any real
handler or network round trip this is noise, but it is a measured number, not a promise.

`cargo test -p nestrs-bench --test soak` drives every protective interceptor with mixed concurrent traffic (3 s by
default; `SOAK_SECS=600 SOAK_WORKERS=64 cargo test -p nestrs-bench --release --test soak -- --nocapture` for a real
soak) and checks the contracts from outside: only allowed statuses, the bulkhead never over capacity, an idempotency
key never runs a write twice, the cache stays bounded and spares the handler, the circuit breaker opens and recovers,
metrics count every request exactly once, request ids are unique.

### Mount: raw routes with a controller's protections

`AppBuilder::merge` mounts a raw axum router with the app-wide pipeline. `Mount` adds, by builder, what a controller
declares by attribute: guards, interceptors and OpenAPI entries.

```rust
App::builder(AppModule).mount(
    Mount::new(Router::new().route("/webhooks/stripe", post(stripe_webhook)))
        .interceptor(RateLimit::per_minute(120))
        .guard::<BearerAuth>()
        .document(RouteDoc::post("/webhooks/stripe", "stripe_webhook").tagged("Webhooks")
            .request_body(Some(BodyDoc::of::<StripeEvent>()))),
)
```

Order is a controller's: guards, then interceptors, then the handler, whatever order the calls are written in; the
last interceptor added is outermost. Documented routes are marked secured when the mount has a guard (and anonymous
when every guard is optional); undocumented routes stay out of the spec. `Mount::with(|container| ..)` builds the
router from providers.

### Roles and permissions

```rust
#[controller("/posts")]
#[guard(BearerAuth)]                         // authenticates and inserts a `Principal`
impl PostsController {
    #[post]
    #[roles("admin", "author")]              // any of these roles
    async fn create(&self, ..) { .. }

    #[delete("/{id}")]
    #[permissions("posts:delete")]           // all of these permissions
    async fn remove(&self, principal: Principal, ..) { .. }
}
```

- Authentication stays yours: a `Guard` builds `Principal::new(id).with_roles([..]).with_permissions([..])` and puts it in the request
  extensions. Authorization is declarative and identity-source agnostic (static tokens, JWT, sessions).
- No principal: **401**. Too few privileges: **403** (the body never names what was required). Class-level and route-level
  attributes must all hold. Requirements run after `#[guard]`s and before interceptors.
- Permissions are `namespace:action`; a granted `posts:*` covers every `posts:` permission, `*` covers all.
- OpenAPI lists "Requires role admin or author; permission ..." and a 403 response, so the generated oRPC contract types `FORBIDDEN`.
- `KeyBy::Custom` can rate limit per caller: `RateLimit::per_minute(60).by(by_caller())` (see blog-api).

### Multi-binding and health probes

Several providers can contribute to one interface; consumers receive them all as `Arc<Multi<dyn Port>>`.

```rust
#[injectable(contributes = HealthIndicatorPort)]   // or: contribute!(dyn HealthIndicator => DatabaseIndicator)
struct DatabaseIndicator { db: Arc<Database> }

struct HealthChecker { indicators: Arc<Multi<dyn HealthIndicator>>, .. }   // sees every contribution
```

- Contribute from any module: the set is visible everywhere, so nobody exports or imports anything. The member's own dependencies
  still follow normal module visibility.
- Order is registration order (modules in dependency order). A set exists once something contributes; declare
  `Provider::multi::<dyn Port>()` to allow an empty one. Members are singletons without lifecycle hooks.
- Tests replace the whole set: `.override_value(Multi::<dyn HealthIndicator>::new(vec![..]))`.

`nestrs-health` builds on it: `imports = [HealthModule]` gives `GET /health/live` (always 200) and `GET /health/ready`
(200 when every critical indicator is up, otherwise 503 with per-check results in `error.details`). Checks run concurrently,
each under a timeout (default 2 s) with panics contained. A `critical() == false` indicator only reports `degraded`. Once shutdown
starts, readiness answers 503 `draining` so load balancers stop sending traffic before the server closes (`ShuttingDown` extractor).
`HealthConfig { expose_details: false }` hides failure reasons from untrusted callers.

### Rate limiting and timeouts

```rust
#[controller("/users")]
impl UsersController {
    #[post]
    #[guard(AuthGuard)]
    #[interceptor(RateLimit::per_minute(60))]        // per client IP; built once, no provider needed
    async fn create(&self, ..) { .. }
}

app.interceptor(RateLimit::per_second(50).scope("global"))   // outermost: before auth, includes 404s
   .interceptor(Timeout::secs(30))
```

- `RateLimit` is GCRA: a burst of `limit`, then one request per `window / limit`; exact `Retry-After`.
  Keys: `KeyBy::Ip` (IPv6 by /64), `Global`, `Header(..)`, `Custom(..)`. `X-Forwarded-For` is only read after
  `.trust_proxy_headers()`. Counters live behind the `RateLimitStore` port (`MemoryStore` is bounded and fails
  open when full; a failing store also fails open). Share a Redis-backed store across instances with `.scope(..)`.
- Route-level interceptors run *inside* guards, so rejected requests are not counted; use `App::interceptor` to throttle before auth.
- `Timeout` covers producing the response head. It cancels the handler future (handlers must be cancel-safe) and does not cut a streaming body (SSE).
- `listen` serves with connect-info so `KeyBy::Ip` sees the socket address.

Guards, interceptors and pipes are declared dependencies of the controller, so bootstrap checks they
exist and are visible. Filters are app-wide; the first one to return `Some(response)` wins.

## Configuration

`nestrs-config` loads typed, validated settings once at startup, from (lowest priority first) your
type's `serde` defaults, TOML files, `PREFIX_`-prefixed environment variables (`__` nests) and explicit
overrides:

```rust
#[derive(Deserialize, Validate)] #[serde(default)]
struct AppConfig { #[validate(length(min = 1))] api_token: String, .. }

#[module(imports = [ConfigModule::for_root::<AppConfig>(ConfigSource::new().file("app.toml").env_prefix("APP"))])]
```

The module is global: any provider can depend on `Arc<AppConfig>`. Parse errors and *every* validation
failure are reported together and stop startup (`Factory` error naming the config provider).

## OpenAPI

`#[controller]` records each route (method, path, `///` docs as summary/description, path and query
parameters, request and response bodies, guards). Types describe themselves with `utoipa`:

```rust
#[derive(Deserialize, Validate, ToSchema)] struct CreateUserRequest { .. }
app.global_prefix("/api")
   .serve_openapi(ApiInfo::new("Blog API", "1.0"))      // /api/openapi.json and Swagger UI at /api/docs
```

Request bodies come from `Json<T>` / `ValidatedJson<T>` (adds a 422), responses from the return type
(through `Result`, tuples and `Vec<T>`), guarded routes get the bearer-auth scheme and a 401.
`#[status(201)]` declares the success status: a handler that answers a plain `200 OK` answers 201 instead
(a handler that picks its own status, or fails, is left alone), and the document shows it. `#[status(204)]` on
a `Result<(), _>` handler gives `204 No Content`. `#[response(404, "No such post")]` documents an extra error
response; it replaces the generic description for that status. Without `#[status]`, a `StatusCode` in the
return type means the verb's usual 201/204. A type without `ToSchema` still compiles; it is documented
without a schema. Your crate needs a direct `utoipa` dependency for the derives. Call `app.openapi(&info)`
to get the document as a value.

## Testing with nestrs-testing

```rust
let app = TestApp::builder(AppModule)
    .override_value(AppConfig { api_token: "t".into(), ..Default::default() })
    .mock::<UserRepositoryPort>(Arc::new(FakeUsers::default()))   // or .override_instance::<dyn T>(..)
    .start().await;                                      // try_start() returns the startup error instead

let user: Value = app.client()
    .post("/users").bearer("t").json(&json!({ "name": "Ada", "email": "ada@example.com" }))
    .send().await
    .assert_status(StatusCode::CREATED)
    .json();
app.resolve::<Trace>();                                  // inspect a fake afterwards
app.shutdown().await;                                    // run destroy hooks
```

Requests go straight into the router in-process: no sockets, ports or sleeps. Failures show the
response body.

## How the SOLID principles show up

- **Single responsibility.** Each crate and each type has one job: `nestrs-core` wires,
  `nestrs-macros` reads declarations, `nestrs-http` speaks HTTP. In an application, a
  controller translates HTTP, a service holds rules, a repository persists, a guard
  authorises, and domain errors stay free of HTTP (`From<UserError> for HttpError` is the
  single seam). `graph.rs` validates, `bootstrap.rs` builds, `container.rs` stores.
- **Open/closed.** New behaviour is added, not edited in. A new transport implements
  `Transport` and ships `ControllerEntry` values; `nestrs-core` never changes. New guards,
  providers and modules are new types. `#[module]` is just sugar over `ModuleDef`'s public
  builder, so modules can be written by hand or generated.
- **Liskov substitution.** Anything bound with `#[injectable(provides = UserRepositoryPort)]` is
  interchangeable: services only see `UserRepositoryPort` and the trait documents its
  contract. The example's tests run the same service against the in-memory adapter, and the
  integration suite runs the full stack against it; a Postgres adapter drops in the same way.
- **Interface segregation.** The framework contracts are small and separate: `Injectable`
  (build me), `Module` (describe me), `Controller` (how to mount me), `Transport` (what
  mounting yields), `Guard` (one `check` method). Nothing forces an implementor to carry
  methods it does not use. Example ports follow suit (`TokenVerifier` has one method).
- **Dependency inversion.** High-level code depends on abstractions and a composition root
  picks implementations. `RegisterUser` holds `UserRepositoryPort`; `BearerAuth` holds
  `TokenVerifierPort`; the `provides = ..` attribute or provider function in the composition
  layer is the only place a concrete type is named. Even the framework follows this: `nestrs-core` depends on no HTTP crate, and
  the HTTP crate plugs in through the `Transport` trait. Overrides are dependency inversion
  put to work: tests replace an abstraction's implementation without editing any module.

## Clean architecture in the example

`examples/blog-api` is organised in concentric layers; source dependencies point inward only.

| Layer | Contains | May depend on |
|---|---|---|
| `domain` | entities, value objects that make invalid states unrepresentable (`Email::parse`), repository **ports** | nothing |
| `application` | use cases (one `#[injectable]` struct, one `execute`), the `TokenVerifier` port, `ApplicationError` | domain |
| `infrastructure` | adapters implementing the ports (`#[injectable(provides = ..)]`): repositories, token verifier, config, seeder | domain, application |
| `presentation` | controllers, request/response DTOs, `by_caller()`, the one `ApplicationError -> HttpError` mapping | application, domain |
| `composition` | `#[module]`s and `#[provider]` functions binding ports to adapters, listing use cases and controllers | everything |

Choices worth knowing:

- **The inner rings touch the framework only through declarations.** `domain` may use `#[port]` and
  `application` `#[port]` and `#[injectable]`, which are annotations on traits and structs; no framework
  type, no HTTP, no serde. A use case is a struct of ports plus `execute`, and its unit tests build it with a
  struct literal (`RegisterUser { users }`). `tests/architecture.rs` allow-lists exactly those lines.
- **Ports live with their consumers' owner**: repository traits in `domain`, `TokenVerifier` in
  `application`. Adapters implement them, so the arrow points inward.
- **Use cases depend on ports, not on each other.** `PublishPost` checks the author through
  `UserRepository`, not through the users use cases.
- **Entities never cross the wire.** DTOs convert to commands and from entities, so the JSON shape and
  the domain model evolve separately. DTO `validator` rules give per-field 422s; the domain enforces the
  same invariants again, so a use case is safe from any caller.
- **Repositories return `Result<_, RepositoryError>`**, so a real database's failures reach the client
  as a logged 500 instead of a panic.
- **The rule is a test.** `tests/architecture.rs` scans each layer's source and fails on an outward
  import (or a framework import in `domain`/`application`).
- Application tests use fakes defined in that layer, never infrastructure.

## Key design decisions

1. **Startup validation instead of runtime reflection.** Rust has no reflection, so
   dependencies are declared (derived from field types by `#[injectable]`) and the graph is
   checked once. The trade-off is honest: module visibility is enforced at *startup*, not by
   the compiler.
2. **`Arc<Arc<T>>` inside `dyn Any`.** `dyn Trait` is unsized and cannot be stored as
   `dyn Any` directly; storing the sized `Arc<dyn Trait>` makes `resolve::<dyn Trait>()` and
   `resolve::<Concrete>()` uniform.
3. **Controllers are providers, mounted later.** They get the same dependency and visibility
   rules for free. Mounting happens after the whole graph is built, which is when guards can
   be resolved.
4. **A transport-neutral core.** `ControllerEntry` stores a type-erased mount function plus a
   transport id; `MountedController::for_transport::<T>()` hands the right artefact to the
   right adapter. This is what keeps gRPC/CLI/queue adapters possible without touching core.
5. **Thin macros, fat generics.** The macros emit trait impls and a few lines of routing glue;
   all logic is ordinary code that is unit-testable and keeps compile times and error
   messages sane.
6. **One error envelope.** `HttpError` renders `{"error":{"code","message","details"}}` for
   guards, validation, domain errors and unknown routes alike. Internal errors are logged,
   never leaked.
7. **Async providers via RPITIT.** `Injectable::build` returns `impl Future + Send`, so
   pools and remote config can be built with `.await` without `async_trait` in the core API.

## Workspace layout

```
nestrs/
├── Cargo.toml                    workspace: shared versions, lints, dependency table
├── crates/
│   ├── nestrs-core/              no HTTP, no macros: the DI + module engine
│   │   ├── src/
│   │   │   ├── lib.rs            public surface and crate docs
│   │   │   ├── token.rs          Token: TypeId identity (works for dyn Trait) + readable names
│   │   │   ├── error.rs          DiError: every wiring failure, with module/type names
│   │   │   ├── container.rs      Container: the singleton store; resolve::<T: ?Sized>()
│   │   │   ├── dependency.rs     Dependency (Arc<T> handles / port aliases), ProviderSource, macro probes
│   │   │   ├── provider.rs       Injectable, Provider (of / bind / value / instance / factory), bind!
│   │   │   ├── lifecycle.rs      OnModuleInit / OnModuleDestroy, Shutdown
│   │   │   ├── scope.rs          Lifetime, RequestScope (per-request container), ScopedProviders
│   │   │   ├── overrides.rs      Overrides: replacement providers applied before validation
│   │   │   ├── module.rs         Module, ModuleDef builder, ModuleRef
│   │   │   ├── transport.rs      Transport, Controller, ControllerEntry, MountedController
│   │   │   ├── graph.rs          collect -> validate -> topological order (pure, no I/O)
│   │   │   └── bootstrap.rs      runs the plan: build in order, then mount controllers
│   │   └── tests/
│   │       ├── di.rs             container, validation and ordering via the public API (no macros)
│   │       ├── composition.rs    overrides, configured modules, global modules
│   │       ├── lifecycle.rs      init/destroy ordering, failures, idempotent shutdown
│   │       └── scopes.rs         request/transient lifetimes and the lifetime rule
│   ├── nestrs-macros/            proc-macro crate (Rust requires it to be separate)
│   │   └── src/{lib.rs, injectable.rs, controller.rs, module.rs, util.rs}
│   ├── nestrs-http/              Axum adapter; the only crate that knows axum
│   │   └── src/
│   │       ├── lib.rs · app.rs   App, AppBuilder (overrides, filters), serve_openapi
│   │       ├── controller.rs     HttpTransport, HttpController, mount
│   │       ├── guard.rs · interceptor.rs · pipe.rs · filter.rs   the request pipeline
│   │       ├── scope.rs          request scopes, Inject<T>, RequestInfo, HttpModule
│   │       ├── openapi.rs        RouteDoc, utoipa-based document builder
│   │       └── validation.rs · error.rs · path.rs
│   ├── nestrs-kit/      reusable plumbing: Service (the `main`), TimingInterceptor, ErrorLogFilter,
│   │                    RequestContext, Pagination + PaginationPipe, KitModule
│   ├── nestrs-interceptors/ circuit breaker, bulkhead, retry, cache, ETag, single-flight, idempotency,
│   │                    request id, metrics, trace context, audit, security headers, tenant/locale/version
│   ├── nestrs-redis/    RedisConnection, rate limit / cache / idempotency stores, RedisIndicator,
│   │                    RedisEventStream, RedisJobLock, RedisQueueBackend, RedisValueStore
│   ├── nestrs-jobs/     Job + Schedule (interval/cron), JobLock, Queue + QueueHandler + QueueBackend, JobsModule
│   ├── nestrs-sea-orm/  Database (lifecycle), Executor, Transactional, UnitOfWork, Migrations port,
│   │                    DatabaseIndicator, SeaOrmModule
│   ├── nestrs-cache/    ValueStore port, MemoryValueStore, ServiceCache (single-flight read-through)
│   ├── nestrs-cli/      the `nestrs` command: `new`, `generate feature` (pure templates, Workspace port)
│   ├── nestrs-bench/    overhead benchmarks vs plain axum, full-stack soak test
│   ├── nestrs-jwt/      JwtAuthenticator (Authenticator port), KeySource + StaticKeys (HMAC/PEM/JWKS), claim mapping
│   ├── nestrs-health/   HealthIndicator port, HealthChecker, /health/live + /health/ready, HealthModule
│   ├── nestrs-config/            ConfigSource (defaults/files/env), ConfigModule::for_root::<T>
│   ├── nestrs-testing/           TestApp, TestAppBuilder, TestClient, TestResponse
│   └── nestrs/                   facade: re-exports + prelude; what apps depend on
│       ├── src/lib.rs
│       └── tests/
│           ├── macros.rs         end-to-end: macros -> bootstrap -> router
│           ├── modules.rs        #[module] expressions, global flag, App::builder overrides
│           ├── http_features.rs  scopes, pipes, interceptors, filters through the macros
│           ├── openapi.rs        generated documents
│           └── ui.rs + ui/       trybuild runner; pass/ and fail/ cases with .stderr snapshots
└── examples/blog-api/            reference application, in clean architecture
    ├── src/
    │   ├── lib.rs · main.rs      library (testable) + thin binary
    │   ├── domain/               entities, value objects (Email, PostTitle, ..), repository ports,
    │   │                         DomainError / RepositoryError. No framework, no serde.
    │   ├── application/          use cases (RegisterUser, PublishPost, ..), TokenVerifier port,
    │   │                         ApplicationError. Plain structs with `new(..)`; fakes in testing.rs
    │   ├── infrastructure/       adapters: persistence/ (in-memory + SeaORM repos, entities and
    │   │                         sea_orm/migrations/; the pool comes from nestrs-sea-orm), auth.rs, config.rs, seed.rs
    │   ├── presentation/http/    controllers, DTOs (ToSchema), kit Pagination -> domain Page,
    │   │                         ApplicationError -> HttpError
    │   └── composition/          nestrs modules: Persistence (picks adapters from config), Auth,
    │                             Users, Posts, App (imports nestrs-kit's KitModule)
    └── tests/
        ├── api.rs                black-box tests: API, pagination pipe, interceptor, scopes, OpenAPI
        ├── overrides.rs          overrides, invalid config, the seeder as an init hook
        ├── sea_orm.rs            the same app on SQLite through SeaORM, selected by config
        ├── migrations.rs         migrations: applied once, rollback, legacy adoption, Postgres (opt-in)
        └── architecture.rs       fails if an import points outward
```

**Why this shape.**

- *Crates split along dependency direction:* `core` <- `http` <- `nestrs`, with `macros`
  beside them. `core` can be reused (or tested) with no web stack; the proc-macro crate must
  be separate; the facade gives users one dependency and one version to track.
- *Feature folders, not layer folders (`users/`, `posts/` instead of `controllers/`,
  `services/`).* Everything that changes together lives together, a feature's public surface
  is its `mod.rs` re-exports plus its module's `exports`, and a feature can be extracted into
  its own crate by moving a folder.
- *Identical file names inside each feature* (`entity`, `dto`, `error`, `repository`,
  `service`, `controller`, `module`) make any feature navigable at a glance, and keep each
  file to one responsibility.
- *Library plus thin binary in the example:* tests drive `AppModule` directly without a
  socket; `main.rs` only reads the bind address, applies layers and serves.

## Running it

```sh
cargo test --workspace
BLOG_API_TOKEN=secret BLOG_SEED_DEMO_DATA=true cargo run -p blog-api   # 127.0.0.1:3000
# on a database instead of memory (SQLite file or Postgres):
BLOG_STORAGE=database BLOG_DATABASE_URL='sqlite://blog.db?mode=rwc' cargo run -p blog-api
# optional blog-api.toml in the working directory; BLOG_* environment variables override it
# migrations run at start-up; BLOG_MIGRATE_ON_START=false leaves them to a separate step

open http://localhost:3000/api/docs                  # Swagger UI; the document is /api/openapi.json

curl localhost:3000/api/health/live
curl localhost:3000/api/health/ready
curl -X POST localhost:3000/api/users \
     -H 'authorization: Bearer secret' -H 'content-type: application/json' \
     -d '{"name":"Ada","email":"ada@example.com"}'
```

## Database: nestrs-sea-orm

`nestrs-sea-orm` is the persistence kit: the part of "use SeaORM" that is the same in every service.

| Piece | Job |
|---|---|
| `DatabaseConfig` | how to connect; deserialisable, builder methods, `enabled(false)` runs without a database |
| `Database` | the pool as a provider: connects, applies migrations on init, closes at shutdown |
| `Executor` | the pool *or* the request's transaction; implements `ConnectionTrait`, so `Entity::find().one(&db)` works |
| `Transactional` | interceptor: one transaction per request |
| `UnitOfWork`, `Database::in_transaction` | explicit transactions for jobs and scripts |
| `Migrations`, `SeaMigrator<M>` | the migration port and its adapter for a `sea-orm-migration` migrator |
| `DatabaseIndicator` | the `database` readiness check |
| `SeaOrmModule` | wires all of it |

```rust
#[module(
    imports = [SeaOrmModule::with_migrator::<Migrator>(database_config())],
    providers = [user_repository, post_repository],
    exports = [UserRepositoryPort, PostRepositoryPort, Transactional],
)]
pub struct PersistenceModule;

// A repository never knows whether a transaction is active:
let db = self.database.executor().await?;
user::Entity::insert(row).exec(&db).await?;

// Atomic writes: add the interceptor to the route.
#[post]
#[interceptor(Transactional)]
async fn create(&self, ..) -> Result<.., HttpError> { .. }
```

How transactions behave:

- The transaction opens **lazily**, the first time a repository asks for an `Executor`, so routes that
  never touch the database send no `BEGIN`.
- A `2xx`/`3xx` response **commits**; a `4xx` or `5xx` **rolls back**. Returning an error from a
  handler undoes its writes with no code for it. If the commit itself fails the client gets a `500`.
- It lives in a Tokio task-local, so it follows the request. Work moved to `tokio::spawn` does not see
  it and uses the pool.
- `Database::in_transaction` inside such a request joins the request's transaction; outside one it
  begins, commits on `Ok` and rolls back on `Err`.
- With in-memory SQLite the pool has one connection, so other users of the database wait while a
  transaction is open. Real databases have no such limit.

Migrations live in your crate (`infrastructure/persistence/sea_orm/migrations/`, one file per change,
registered in `Migrator`) and run once per database, recorded in `seaql_migrations`.

- `Database` applies pending migrations in its init hook, before anything uses the repositories. Set
  `migrate_on_start(false)` (the blog: `BLOG_MIGRATE_ON_START=false`) when migrations are a separate
  deployment step, then call `Database::migrate()` yourself.
- `Database::roll_back(n)` reverts the last `n` migrations; `applied_migrations()` lists what ran.
- `tests/migrations.rs` in the blog covers SQLite (restart, rollback and re-apply, legacy adoption,
  switched off). Set `BLOG_TEST_POSTGRES_URL=postgres://user@host/db` to run the same migrations on a
  real Postgres; that test resets the database it is given, so point it at a scratch one.

The crate enables no driver; turn on its `sqlite`, `postgres` or `mysql` feature.

## Known limits

- Mounted routes are documented by hand (`RouteDoc`); there is no attribute to read them from.
- Redis-backed tests need a local `redis-server`; set `REQUIRE_REDIS=1` (CI does) to fail instead of skipping.
- The Redis queue `reserve` polls (50 ms) rather than blocking, so a push is picked up within about that long.
- The CLI edits what it can parse and reports the rest; it does not reformat attribute arguments, so a long
  `#[module(..)]` line stays long.
- `Transactional` is per request task; spawned tasks use the pool (see above).

## Typed Server-Sent Events and a React client

Return `EventStream<T>` from a handler to stream typed events:

```rust
#[get("/events")]
async fn events(&self) -> EventStream<PostEventDto> { /* a Stream of events */ }
```

Every stream is `flush`, then `data: <json>` messages, then `event: close` (or
`event: error`), the protocol oRPC event iterators speak. `T` is documented in the
OpenAPI spec as `text/event-stream`. Open streams are closed when the app shuts
down, so graceful shutdown completes. `examples/react-orpc` is a React + oRPC
(OpenAPI link, contract-first) + TanStack Query client with e2e tests against the
real binary; see its README. Its client contract is generated from the server's OpenAPI document
(`npm run gen`), so Rust types, spec and TypeScript types cannot drift apart silently.

### Resumable streams

Give messages an id and read the `Last-Event-ID` header to resume after it:

```rust
#[get("/events")]
async fn events(&self, last: LastEventId) -> EventStream<PostEventDto> {
    let after = last.parse::<EventCursor>(); // None when absent or malformed
    EventStream::from_events(/* Event::with_id(cursor, dto) items, replay first */)
}
```

`blog-api` keeps a bounded history behind the `PostEvents` port; `subscribe(after)`
replays what was missed and then goes live with no gap and no duplicate. Cursors carry
a per-run epoch so ids from before a restart are ignored rather than misread.
