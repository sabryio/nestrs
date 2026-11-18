# purlin-health

Liveness and readiness probes with pluggable health indicators.

A `HealthIndicator` port, critical and non-critical checks, per-check timeouts, and indicators that switch themselves off when their backend is not configured.

Add the module, then contribute one indicator per backend.

```toml
[dependencies]
purlin-health = "0.1"
```

Part of [purlin](https://github.com/sabry-awad97/purlin). Licensed under MIT or Apache-2.0, at your option.
