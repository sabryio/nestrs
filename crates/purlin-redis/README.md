# purlin-redis

Redis adapters for purlin ports: rate limiting, response cache, idempotency keys, job locks and queues, value store, event bus.

Each adapter implements a port from another crate, so switching between memory and Redis is a provider change, not a code change.

Needs a Redis server (Lua scripting is used for atomic operations).

```toml
[dependencies]
purlin-redis = "0.1"
```

Part of [purlin](https://github.com/sabry-awad97/purlin). Licensed under MIT or Apache-2.0, at your option.
