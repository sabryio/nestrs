# purlin-cache

Service-level read-through caching: a value store port and a single-flight cache.

`ServiceCache::get_or_load` runs the loader once per key even under concurrency, never caches errors and fails open if the store is down.

Use `MemoryValueStore` locally and `purlin-redis` in production.

```toml
[dependencies]
purlin-cache = "0.1"
```

Part of [purlin](https://github.com/sabry-awad97/purlin). Licensed under MIT or Apache-2.0, at your option.
