# fx-axum

A plain [axum](https://docs.rs/axum) service wired with **rust-fx only**. No purlin.

```text
cargo run -p fx-axum
curl -X POST localhost:3000/greetings/Ada
curl localhost:3000/greetings
```

Ctrl-C stops it gracefully: the server drains in-flight requests, then the app unwinds.

## Layout

| File | Role |
|---|---|
| `src/domain.rs` | The use case (`GreetingService`) and its ports (`Clock`, `GreetingStore`). No axum, no adapters. |
| `src/infra.rs` | Adapters: `SystemClock`, `InMemoryStore`, bound to their ports with `#[fx::bind]`. |
| `src/web/mod.rs` | The `Controller` port: "contributes routes". |
| `src/web/{greetings,health}.rs` | Controllers, joined to the `Controller` group with `#[fx::bind(group)]`. |
| `src/web/server.rs` | The server as a component: async constructor binds, start hook serves, stop hook drains. |
| `src/modules.rs` | The three `#[fx::module]`s and `app()`, the composition root. |
| `tests/http.rs` | Real-socket end-to-end test, busy-port failure, wiring validation, framework-free domain test. |

## What it shows

- **Constructor injection** with plain functions; the graph is checked before anything runs.
- **Ports without `dyn`**: `Arc<ClockPort>`; tests swap one with `App::replace::<ClockPort>(..)`.
- **Open/closed**: a new endpoint group is one controller plus one line in `WebModule`; the server does not change.
- **Lifecycle**: the listener is bound in the constructor (a busy port fails the start, with a clear error) and stopped by a hook.
- **Module visibility**: `GreetingModule` keeps its store private and exports only the service.

## SOLID, in this example

- **S**: domain, adapters, routes and serving each live in one file group.
- **O**: controllers are discovered through the `Controller` group, so adding one never edits the server.
- **L**: any `Clock` or `GreetingStore` can stand in for the real one (the tests do).
- **I**: `Clock`, `GreetingStore` and `Controller` are one-method ports.
- **D**: `GreetingService` and the server depend on ports; `modules.rs` is the only place that names adapters.
