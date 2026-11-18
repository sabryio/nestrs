# purlin-jobs

Scheduled tasks and background queues with pluggable locks and backends.

`Job` (interval or cron), `QueueHandler` with retries, back-off and dead letters, lease-based at-least-once delivery, and a `JobRunner` that stops cleanly on shutdown. In-memory adapters are included; `purlin-redis` adds Redis.

Import `JobsModule` and implement `Job` or `QueueHandler`.

```toml
[dependencies]
purlin-jobs = "0.1"
```

Part of [purlin](https://github.com/sabry-awad97/purlin). Licensed under MIT or Apache-2.0, at your option.
