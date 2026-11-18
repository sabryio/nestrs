# purlin-http

Axum adapter for purlin: controllers, guards, validation, WebSockets, SSE, OpenAPI and the `App` bootstrap.

Everything that turns a module graph into an `axum::Router`, including graceful shutdown that also closes WebSockets and SSE streams.

Re-exported by `purlin` as `purlin::http`.

```toml
[dependencies]
purlin-http = "0.1"
```

Part of [purlin](https://github.com/sabry-awad97/purlin). Licensed under MIT or Apache-2.0, at your option.
