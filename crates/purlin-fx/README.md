# purlin-fx

Hosts a purlin module graph, and its HTTP server, inside an `fx::App`:

- **rust-fx** (`fx`): dependency injection, modules, lifecycle, a graceful runtime.
- **purlin**: modules, controllers, request scopes, HTTP. Its core already runs on fx; this
  crate makes the purlin graph one *component* of a larger fx app, sharing its lifecycle,
  signals and shutdown path.

```rust
fx::App::new()
    .component::<Visits>()                      // plain fx components
    .options(purlin_fx::hosted::<WebModule>())  // a purlin module graph, as one fx component
    .options(purlin_fx::serve("0.0.0.0:3000"))  // purlin's HTTP server, as another
    .run()
    .await?;
```

- `hosted::<M>()` bootstraps the purlin graph during fx's start phase (a wiring mistake fails
  startup) and runs its `#[on_destroy]` hooks, in reverse order, during stop. `Hosted` is an
  ordinary fx component: take `Arc<Hosted>` to reach purlin singletons.
- `serve(addr)` / `serve_with(addr, |app| ..)` (feature `http`, default) bind at construction,
  serve on start and drain in-flight requests on stop. Ctrl-C and SIGTERM are handled by fx.

See `examples/purlin-on-fx` for a complete program.

## Layout

```text
src/lib.rs        Hosted, hosted(), and (feature `http`) HttpServer, serve(), serve_with()
tests/hosting.rs       graph lifecycle inside an fx app (headless)
tests/hosting_http.rs  controllers served over a real socket, drained on stop
```
