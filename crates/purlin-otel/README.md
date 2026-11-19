# purlin-otel

OpenTelemetry for [purlin](https://github.com/sabry-awad97/purlin): OTLP export, HTTP server spans, and
W3C trace propagation across queues.

The framework crates emit plain `tracing` spans and know nothing about OpenTelemetry. This crate is the
adapter, so you opt in with one dependency and the rest of the framework stays backend-agnostic.

| Piece | Role |
| --- | --- |
| `Telemetry` | Starts the pipeline (OTLP/HTTP, parent-based ratio sampling, resource) and gives you a `tracing-subscriber` layer |
| `HttpTracing` (feature `http`, default) | Interceptor: server span `METHOD /route/{template}`, continues an incoming `traceparent`, echoes one back |
| `OtelPropagation` | Implements `purlin-jobs`' `TracePropagation` port: queued messages carry the trace |
| `OtelConfig` | Plain data: service name/version, endpoint, sample ratio, timeout, resource attributes |

```rust
let telemetry = Telemetry::init(&OtelConfig::new("blog").with_endpoint("http://otel:4318/v1/traces"))?;
tracing_subscriber::registry().with(telemetry.layer()).with(fmt::layer()).init();

// queues: JobsConfig { propagation: Some(Arc::new(OtelPropagation::new())), ..Default::default() }
// http:   #[interceptor(HttpTracing)] on a controller, or Service::interceptor(HttpTracing::new())
telemetry.shutdown(); // flush on the way out
```

## Spans you get

| Span | Kind | Emitted by |
| --- | --- | --- |
| `GET /posts/{id}` | server | `HttpTracing` |
| `job <name>` | internal | `purlin-jobs` runner |
| `<queue> process` | consumer (child of the enqueuing span) | `purlin-jobs` worker |
| `outbox.publish` | producer | `purlin-outbox` relay |

Failures set `otel.status_code = ERROR` (5xx responses, failed jobs and handlers, failed publishes).

## Testing

`Telemetry::with_exporter(&config, InMemorySpanExporter::default())` exports synchronously and touches no
global state; see `tests/otel.rs`.

## Known gap

An outbox row does not store trace context, so the relay's `outbox.publish` span starts a new trace; from the
queue onwards the trace is linked.

Licensed under MIT or Apache-2.0.
