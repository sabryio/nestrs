# nestrs-interceptors

A catalogue of production interceptors: resilience, caching, idempotency, observability, HTTP hygiene.

Circuit breaker, bulkhead, retry, timeout, rate limit, response cache, idempotency keys, request ids, metrics (with a Prometheus registry), W3C trace context, audit log, ETag, security headers and more, each behind a small port so storage can be swapped.

Attach per route or per controller, or through `Mount` for routes you build yourself.

```toml
[dependencies]
nestrs-interceptors = "0.1"
```

Part of [nestrs](https://github.com/sabry-awad97/nestrs). Licensed under MIT or Apache-2.0, at your option.
