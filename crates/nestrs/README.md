# nestrs

The crate applications depend on: modules, dependency injection and controllers on top of Axum.

Re-exports `nestrs-core`, `nestrs-http` and `nestrs-macros`, so one dependency gives you `#[injectable]`, `#[controller]`, `#[module]`, guards, interceptors, validation and OpenAPI generation.

Start here. Add the satellite crates (`nestrs-jobs`, `nestrs-cache`, `nestrs-redis`, ...) only when you need them.

```toml
[dependencies]
nestrs = "0.1"
```

Part of [nestrs](https://github.com/sabry-awad97/nestrs). Licensed under MIT or Apache-2.0, at your option.
