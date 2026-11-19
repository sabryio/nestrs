# rust-fx

An application framework for Rust in the spirit of Uber's [fx](https://uber-go.github.io/fx/):
dependency injection, modules, lifecycle hooks and a graceful runtime. **No web stack is
attached**: workers, CLIs, schedulers, queue consumers and servers all start the same way.

The package is `rust-fx`; the library is `fx`.

```rust
use std::sync::Arc;

pub struct Clock;

#[fx::component]
impl Clock {
    #[fx::constructor]
    pub fn new() -> Self { Self }
}

#[fx::module(providers = [Clock], exports = [Clock])]
pub struct ClockModule;

fn announce(clock: Arc<Clock>) { /* runs once at startup */ }

#[fx::main]                       // like #[tokio::main], on the runtime fx re-exports
async fn main() -> fx::Result<()> {
    let code = fx::App::new()
        .module::<ClockModule>()
        .invoke(announce)
        .run()          // start, wait for SIGINT/SIGTERM or a shutdown request, stop
        .await?;
    std::process::exit(code)
}
```

### No direct `tokio` dependency needed

`fx::tokio` re-exports the runtime, and `#[fx::main]` accepts the same options as
`#[tokio::main]` (`flavor = "current_thread"`, `worker_threads = 2`). Spawn and sleep through
`fx::tokio::spawn` / `fx::tokio::time::sleep`. Features beyond `rt`, `rt-multi-thread`,
`sync`, `time`, `signal` and `macros` (such as `net`) still need `tokio` in your manifest;
Cargo unifies them. `#[tokio::test]` also needs a direct (dev-)dependency, because that macro
names the crate itself.

### Binding ports to adapters

```rust
trait Clock: Send + Sync { fn tick(&self) -> u32; }

#[fx::bind]                       // declares `CounterClock: fx::Bound<dyn Clock>`
impl Clock for CounterClock {
    fn tick(&self) -> u32 { self.0.fetch_add(1, Ordering::SeqCst) }
}

#[fx::component]
impl CounterClock {
    #[fx::constructor]
    pub fn new() -> Self { Self::default() }
}

#[fx::module(providers = [CounterClock], exports = [dyn Clock])]   // publishes the port only
pub struct TimeModule;
```

Wherever a constructor of the plain `CounterClock` is registered, the `dyn Clock` provider
is registered next to it, in the same scope; nothing spells the cast. `#[fx::bind(group)]`,
`#[fx::bind(group = "jobs")]` and `#[fx::bind(name = "x")]` choose where the interface value
goes. Forgetting the attribute is a compile error (`CounterClock is not declared as an
implementation of dyn Clock`), because explicit binding requires `Bound`.

See `examples/worker.rs` for a complete headless worker (`cargo run -p rust-fx --example worker`).

## fx, translated

| fx (Go) | rust-fx |
|---|---|
| `fx.Provide(f)` | `.constructor(f)`: any `fn` (sync or `async`, up to 12 parameters) |
| `fx.Invoke(f)` | `.invoke(f)`, or `#[fx::invoke]` in a component |
| `fx.Decorate(f)` | `.decorate(f)`, or `#[fx::decorate]` in a component |
| `fx.Supply(v)` | `.supply(v)` / `.supply_arc::<dyn T>(arc)` |
| `fx.As(new(I))` | `#[fx::bind]` on `impl I for T` (automatic), `fx::bind::<dyn I, T>()` or `fx::constructor(f).bind::<dyn I>()` (explicit) |
| `fx.Module` / `fx.Private` | `#[fx::module(imports, providers, exports, ...)]`; private by default, `exports` to share |
| `fx.Options(...)` | `fx::Options::new().constructor(..).invoke(..)`, `.merge(..)` |
| `fx.In` / `fx.Out` | `#[derive(fx::Params)]` for inputs; return a `Result`/`Arc` for outputs |
| `name:"x"` | `Named<Tag, T>` with `fx::tag!(Tag = "x")`; `#[fx::constructor(name = "x")]` |
| `group:"x"` | `Group<T>` / `Group<T, Tag>`; `#[fx::constructor(group)]` |
| `optional:"true"` | `Option<Arc<T>>` |
| `fx.Lifecycle`, `fx.Hook` | `Lifecycle` (`on_start`, `on_stop`, `append(Hook)`), `#[fx::on_start]` / `#[fx::on_stop]` |
| `fx.Shutdowner` | `Shutdowner` (`shutdown`, `shutdown_with(code)`) |
| `fx.StartTimeout` / `StopTimeout` | `.start_timeout(d)` / `.stop_timeout(d)` (default 15 s) |
| `fx.WithLogger` | `.observer(impl Observer)`; default `fx::Tracing` |
| `fx.ValidateApp` | `App::validate()` |
| `app.Start / Stop / Done / Run` | `start()`, `stop()`, `done()`, `run()` |
| `fx.Populate` | `App::get::<T>()` on a running app |
| `fxtest`, `fx.Replace` | `fx::test::App` (feature `test`), `.replace::<T>(value)` |

## Semantics worth knowing

- **Lazy construction.** Only what an invoke (transitively) needs is built. Everything is
  still *validated*, so an unused provider with a missing dependency is an error.
- **Errors name the chain.** `Database` missing for `Repo` required via
  `invoke serve -> Service -> Repo`, with each provider's `file:line`. Cycles print the loop.
- **Rollback.** If a start hook fails, hooks that started are stopped in reverse order.
  Stop runs every hook even if one fails and reports all failures.
- **Scopes.** Providers at the app root are visible everywhere. A module's providers are
  visible inside it, and to importers only if exported. A module-local provider shadows a
  root one. Decorators apply to the scope they are declared in; root decorators apply
  everywhere. Groups are app-wide.
- **Returning `Self`.** `-> Self` works for types marked `#[fx::component]` or
  `#[derive(fx::Component)]`; for foreign types return `Arc<T>`. The container always
  stores and injects `Arc<T>`.
- **Interfaces** must be `Send + Sync` (declare them as supertraits) so `dyn Port` can be
  shared across tasks.
- `Lifecycle` and `Shutdowner` are taken *by value* (cheap handles), and hooks take no
  arguments; bound them with the start/stop timeouts instead.

## Design: SOLID

- **Single responsibility.** One module, one job: `key` (identity), `extract` (parameters),
  `func` (function-to-constructor adapters), `descriptor` (what a registration looks like),
  `options`/`module` (composition), `graph` (planning and validation), `lifecycle`,
  `shutdown`, `observer`, and `app` (the runtime that executes a plan). The planner never
  runs code; the runtime never reasons about scopes.
- **Open/closed.** New kinds of parameter implement `Extract` (`#[derive(fx::Params)]` is
  one); new observers implement `Observer`; new things a type contributes implement
  `Component::register`. None of that touches the planner or the runtime.
- **Liskov substitution.** `Arc<dyn Port>` and `Arc<Concrete>` are interchangeable behind
  the same keys, `fx::test::App` substitutes for `App`, and `replace` swaps a provider for
  a value without the consumers noticing.
- **Interface segregation.** Functions ask for exactly what they use (`Arc<T>`,
  `Lifecycle`, `Shutdowner`...) rather than a god-container; `Constructor`, `Invocation`,
  `Extract`, `Outcome`, `Observer` and `Module` are each one narrow contract.
- **Dependency inversion.** Application code depends on ports (`dyn Trait`) and is bound
  to adapters (`#[fx::bind]` on the trait impl); the framework depends on the `Extract` and
  `Constructor` abstractions, never on concrete user types.

## Layout

```text
rust-fx/
  src/
    lib.rs          public surface, `tag!`, prelude
    key.rs          Key (type + name + group) and Origin (fn + file:line)
    error.rs        Error, Phase, BoxError
    value.rs        type-erased storage (Arc<dyn Any> holding Arc<T>)
    extract.rs      Extract + Arc/Option/Named/Group/Lifecycle/Shutdowner
    func.rs         Constructor/Invocation blanket impls for fn(..)->R, Component, Outcome
    descriptor.rs   Provide/Decorate/Invoke builders and their erased forms
    bind.rs         Bound (the `#[fx::bind]` declaration), `fx::bind`, link-time registry
    options.rs      Options: ordered list of everything an app is made of
    module.rs       Module, ModuleDef, ModuleRef, Export
    graph.rs        planner: scopes, lookup, decorator chains, cycle/visibility checks
    lifecycle.rs    Hook, Lifecycle, hook registry
    shutdown.rs     Shutdowner, Signal, OS signals
    observer.rs     Event, Observer, Tracing, Silent
    app.rs          App (builder) and Runtime (executes a plan)
    test.rs         fx::test (feature `test`)
  tests/            core (runtime), macros, harness, ui (compile-fail, run on demand)
  examples/         worker.rs, a headless worker
rust-fx-macros/     the macros, as a compiler pipeline (below)
  src/
    lib.rs          the public macros, nothing else
    pipeline.rs     trait Macro (parse -> analyze -> generate) and the driver
    macros.rs       which stage implementations each macro uses
    ast/            stage 1: tokens -> syntax tree (what was written)
    sema/           stage 2: the rules; reports every violation via Diagnostics
    ir.rs           the shared vocabulary: Constructor, Hook, Binding, Module, ...
    codegen/        stage 4: IR -> Rust (cannot fail)
    diagnostics.rs  error accumulation; support/  fx path resolution, syntax helpers
    tests.rs        stage unit tests on token strings
```

One concept per file keeps each reviewable on its own; the dependency arrow runs
`app -> graph -> descriptor/options/module -> func -> extract -> key/error/value`, with
`lifecycle`, `shutdown` and `observer` as leaves, so nothing below `app` knows about the
runtime. The proc-macro crate is separate because a `proc-macro` crate can export nothing
else, and the `macros` feature lets library authors depend on `fx` without compiling `syn`.

## Features

- `macros` (default): `#[fx::component]`, `#[fx::module]`, `#[derive(fx::Params)]`, `#[derive(fx::Component)]`.
- `test`: `fx::test::App` and `Recorder`.

Dependencies: `tokio` (rt, sync, time, signal), `tracing`, `thiserror`. No `hyper`, `axum` or `tower`.

MSRV 1.89. Licensed under MIT OR Apache-2.0.
