# nestrs-health

Liveness and readiness probes with pluggable health indicators.

A `HealthIndicator` port, critical and non-critical checks, per-check timeouts, and indicators that switch themselves off when their backend is not configured.

Add the module, then contribute one indicator per backend.

```toml
[dependencies]
nestrs-health = "0.1"
```

Part of [nestrs](https://github.com/sabry-awad97/nestrs). Licensed under MIT or Apache-2.0, at your option.
