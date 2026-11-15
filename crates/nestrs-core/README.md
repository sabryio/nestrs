# nestrs-core

Transport-agnostic dependency-injection container and module graph.

Tokens, providers (singleton, request-scoped, multi-binding), module imports/exports, lifecycle hooks and test overrides. It knows nothing about HTTP.

Depend on it directly only when writing an adapter for another transport. Applications use `nestrs`.

```toml
[dependencies]
nestrs-core = "0.1"
```

Part of [nestrs](https://github.com/sabry-awad97/nestrs). Licensed under MIT or Apache-2.0, at your option.
