# purlin-core

Transport-agnostic dependency-injection container and module graph.

Tokens, providers (singleton, request-scoped, multi-binding), module imports/exports, lifecycle hooks and test overrides. It knows nothing about HTTP.

Depend on it directly only when writing an adapter for another transport. Applications use `purlin`.

```toml
[dependencies]
purlin-core = "0.1"
```

Part of [purlin](https://github.com/sabry-awad97/purlin). Licensed under MIT or Apache-2.0, at your option.
