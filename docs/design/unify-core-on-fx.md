# Unifying `purlin-core` on `fx`

Status: **in progress** (stages 9-12 done; the macros still expand to purlin-core providers). Decision recorded 2026-11-19.

## Decision and exception

Until now `purlin` and `rust-fx` were independent. That rule is lifted for **one edge**:
`purlin-core` (and the crates above it) may depend on `rust-fx`. `rust-fx` still knows
nothing about purlin. The dependency points one way only: purlin -> fx.

Why: purlin ran a second DI engine (tokens, container, graph, bootstrap) that duplicates
fx's planner, scopes, lifecycle and tests. Two engines mean two sets of bugs.

## Gap analysis

| purlin-core feature | fx today | Plan |
|---|---|---|
| Singleton providers, ordering, cycle/missing detection | planner does this | reuse |
| Module imports/exports, visibility | scopes + exports | reuse |
| `#[module]` instances with config (`for_root`) | missing | **done in stage 9** (`ModuleDef::into_ref`, `Options::module_ref`) |
| Global modules | missing | **done in stage 9** (`ModuleDef::global`) |
| `Overrides` (strict, unknown override is an error) | `Options::replace` + `UnknownReplace` | reuse; map `Overrides` onto it |
| `on_init` / `on_destroy` | `on_start` / `on_stop` | map; check ordering (see risks) |
| `Multi<I>` / `contributes` | `Group<T>` | map `Multi` onto `Group`; keep `Multi` as a thin wrapper |
| `#[port]` / `provides =` | `#[fx::port]` / `#[fx::bind]` | reuse |
| Request and transient lifetimes, `RequestScope`, `seeded` | missing | **done in stage 10**: `Provide::request()/transient()`, `Options::seeded`, `App::scopes()` -> `Scopes::open()` -> `RequestScope`, `Error::ScopeMismatch` |
| Controllers / `Transport` / `MountedController` | not fx's job | stay in purlin; controllers become a Group of mount functions resolved after start |
| Sync `Container::resolve` in `Injectable::build` | extractors | `#[injectable]` expands to an fx constructor; `Container` survives only as a request-time view |
| `TestApp`, `purlin-testing` | fx `test` harness | rebase on fx harness |

## Staged plan

1. **Stage 9 (this one)**: fill the fx gaps that are pure engine features: global modules, configured instances. No purlin behaviour changes.
2. **Stage 10 (done)**: request/transient lifetimes in fx, with the no-singleton-depends-on-scoped check. Limits: scoped values resolve by root-visible key; hooks on scoped constructors do not run.
3. **Stage 11 (done)**: the boundary is drawn where each side is strongest. purlin keeps declaration and validation (module graph, visibility, exports, multi-bindings, overrides, typed `DiError`s in `graph.rs`). `bootstrap` hands the *resolved, ordered singleton list* to `fx::App::eager()` through `fx::Provider::dynamic`; fx builds, runs init hooks once everything exists, and stops in reverse. The built-in build loop and hook runner are deleted. All 614 workspace tests pass unchanged. Differences a user can see: `Shutdown::len`/`is_empty` are gone (the hooks live in fx now), and `Shutdown::run` ignores its container argument (kept for source compatibility).
4. **Stage 12 (done)**: request/transient resolution runs on `fx::Scopes` (`ScopedProviders` wraps it, `RequestScope` wraps `fx::RequestScope`, `Lifetime` is `fx::Lifetime`). Undeclared seeds are still accepted. Next (stage 13): `#[injectable]`/`#[module]` expand directly to fx constructors; delete the old graph/container; retire the `purlin-fx` adapter.
5. **Stage 13**: update independence checks, README, CHANGELOG, tutorial 10.

Each stage must keep the workspace green (fmt, clippy `--all-features`, tests, `check-headless.sh`).

## Risks

- Lifecycle ordering (**resolved, pinned in `rust-fx/tests/ordering.rs`**): fx also runs start hooks only after everything reachable is built, in dependency order, and stops in reverse; a failed start unwinds what had started and not the failing hook; a failing stop does not block the others. The two real differences: fx builds lazily (new `App::eager()` makes it build every singleton, as purlin does), and a second `App::stop` is `Error::NotStarted` where purlin's `Shutdown::run` is a silent no-op (the facade absorbs this).
- `Error` size budget in fx (128 bytes) when new variants land.
- Headless guarantee: purlin-core must stay free of server crates; fx already is.
- `check-headless.sh` currently forbids purlin -> rust-fx. It is updated in stage 13, when the first purlin crate actually depends on fx, so it keeps protecting the other direction (fx -> purlin) throughout.
