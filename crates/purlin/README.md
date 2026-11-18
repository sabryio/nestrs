# purlin

The crate applications depend on: modules, dependency injection and controllers on top of Axum.

Re-exports `purlin-core`, `purlin-http` and `purlin-macros`, so one dependency gives you `#[injectable]`, `#[controller]`, `#[module]`, guards, interceptors, validation and OpenAPI generation.

Start here. Add the satellite crates (`purlin-jobs`, `purlin-cache`, `purlin-redis`, ...) only when you need them.

```toml
[dependencies]
purlin = "0.1"
```

Part of [purlin](https://github.com/sabry-awad97/purlin). Licensed under MIT or Apache-2.0, at your option.
