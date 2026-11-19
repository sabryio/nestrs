# 10. A worker without HTTP

Not every service has a web edge. A queue consumer, a scheduler or a sync daemon needs the same things a web service does
(ports and adapters, wiring, start and stop hooks, a graceful shutdown) and none of the HTTP. For that, use **`rust-fx`**
(library name `fx`): purlin's sibling framework. The two do not depend on each other; you pick the one you need, or both.

## Start one

```text
purlin new sweeper --kind worker --purlin-path <path to the checkout>
cd sweeper
cargo test
cargo run          # runs until Ctrl-C, then stops hooks in reverse order
```

The project has the same layers as a web service, with a different edge:

```text
domain/          ports: Clock, Job
application/     the use case: run every job on a schedule
infrastructure/  adapters: SystemClock, Heartbeat
composition/     fx modules and app()
```

Its dependency tree has `rust-fx` (which brings `tokio`; `#[fx::main]` and `fx::tokio` mean you never list it yourself) and nothing else of ours. No `axum`, no `hyper`, no `purlin`.

## Ports without `dyn`

A port is a trait marked `#[fx::port]`. The attribute adds the `Send + Sync` supertraits and a type alias, `<Trait>Port`,
so no signature ever writes `dyn`.

<!-- source: examples/headless-worker/src/main.rs -->
```rust
#[fx::port]
trait Clock {
    fn tick(&self) -> u32;
}

#[fx::port]
trait Job {
    fn name(&self) -> &'static str;
    fn run(&self, now: u32) -> String;
}
```

## Adapters bind themselves

`#[fx::bind]` goes on the trait impl, where the relationship is written. Wherever the adapter's constructor is registered,
the port is provided next to it; nothing spells a cast. `#[fx::bind(group)]` puts the value in a group instead, which is how
"every job" is expressed.

<!-- source: examples/headless-worker/src/main.rs -->
```rust
#[fx::bind]
impl Clock for CounterClock {
    fn tick(&self) -> u32 {
        self.0.fetch_add(1, Ordering::SeqCst)
    }
}
```

<!-- source: examples/headless-worker/src/main.rs -->
```rust
#[fx::bind(group)]
impl Job for Cleanup {
    fn name(&self) -> &'static str {
        "cleanup"
    }
```

Forgetting the attribute is a compile error that names it: ``CounterClock is not declared as an implementation of
`dyn Clock` ``, with the note to put `#[fx::bind]` on the impl.

## Modules decide what is visible

<!-- source: examples/headless-worker/src/main.rs -->
```rust
#[fx::module(providers = [CounterClock], exports = [ClockPort])]
struct TimeModule;

#[fx::module(providers = [Cleanup])]
struct JobsModule;
```

`TimeModule` exports the port only: the concrete clock is private to the module, so nothing else can depend on it.
Groups are application-wide, so `JobsModule` needs no export.

## The use case is an invoke

The scheduler asks for what it needs. `Lifecycle` registers a stop hook; `Shutdowner` asks the app to end.

<!-- source: examples/headless-worker/src/main.rs -->
```rust
fn drive(clock: Arc<ClockPort>, jobs: Group<JobPort>, journal: Arc<Journal>, life: Lifecycle, stop: Shutdowner) {
    let handle = fx::tokio::spawn(async move {
        for _ in 0..3 {
            let now = clock.tick();
```

`app().run()` builds only what an invoke needs, runs start hooks, waits for Ctrl-C, SIGTERM or `Shutdowner`, then stops in
reverse order and returns an exit code. If any start hook fails, the ones already started are rolled back.

## Testing is replacing

<!-- source: examples/headless-worker/src/main.rs -->
```rust
    #[test]
    fn the_wiring_validates_without_running_anything() {
        app(Arc::default()).validate().unwrap();
    }
```

`validate()` checks the whole graph (missing dependencies, cycles, visibility) without constructing anything.
`.replace::<ClockPort>(Arc::new(Fixed))` swaps an adapter for a test double without touching any module; the harness in
`fx::test` (feature `test`) adds start, stop and lifecycle recording.

## Using both frameworks in one process

A service that needs purlin's controllers and fx's lifecycle depends on both plus the adapter crate `purlin-fx`:

<!-- source: examples/purlin-on-fx/src/main.rs -->
```rust
pub fn app(address: &str) -> fx::App {
    fx::App::new()
        .component::<Visits>()
        .options(hosted::<WebModule>())
        .options(serve(address))
        .invoke(announce)
}
```

`hosted` runs a purlin module graph as one fx component (bootstrapped at start, destroy hooks at stop). `serve` is the HTTP
server as another. Neither framework knows the other: `scripts/check-headless.sh` fails CI if either starts depending on it.

## SOLID, here

* **Single responsibility**: a port says what is needed, an adapter does it, a module decides visibility, `app()` wires.
* **Open/closed**: a new job is one adapter with `#[fx::bind(group)]` and one name in a module; the scheduler is unchanged.
* **Liskov**: any `Clock` can stand in for the real one, which is what `replace` relies on.
* **Interface segregation**: `Clock` has one method; a job sees nothing of the scheduler.
* **Dependency inversion**: the use case depends on `ClockPort` and `JobPort`, never on an adapter.
