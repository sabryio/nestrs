# nestrs-http

Axum adapter for nestrs: controllers, guards, validation, WebSockets, SSE, OpenAPI and the `App` bootstrap.

Everything that turns a module graph into an `axum::Router`, including graceful shutdown that also closes WebSockets and SSE streams.

Re-exported by `nestrs` as `nestrs::http`.

```toml
[dependencies]
nestrs-http = "0.1"
```

Part of [nestrs](https://github.com/sabry-awad97/nestrs). Licensed under MIT or Apache-2.0, at your option.
