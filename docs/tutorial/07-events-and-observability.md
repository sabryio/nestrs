# 7. Events and observability

## Announce a post without losing or inventing it

Publishing a post should tell the rest of the system, but a message sent *before* the commit can describe something that
never happened, and one sent *after* can be lost. The fix is the transactional outbox: record the announcement in the same
transaction as the post, deliver it afterwards, at least once.

The domain only states the need:

<!-- source: examples/blog-api/src/domain/announcements.rs -->
```rust
/// Port: tells the rest of the world that something happened, **reliably**.
///
/// Unlike [`PostEvents`](super::PostEvents) (a live feed that may drop what nobody is
/// listening to), an announcement must not be lost and must not be sent for something that
/// did not happen. The adapter therefore records it inside the same transaction as the write
/// that caused it, and delivery happens afterwards, at least once. Consumers deduplicate.
#[port]
pub trait PostAnnouncements {
    /// Records that `post` was published. Called after the post is stored; if this fails
    /// the whole operation fails and nothing is kept.
    async fn post_published(&self, post: &Post) -> Result<(), RepositoryError>;
}
```

The use case calls it right after storing the post (chapter 3, `post_published`). The database adapter writes a row through
the request's transaction, so a rolled-back request leaves no announcement. A relay (`nestrs-outbox`) polls, claims rows
with leases, publishes them to the queue with retries and backoff, and parks poison messages as dead letters.

The queue and the consumer are wired in one module:

<!-- source: examples/blog-api/src/composition/messaging_module.rs -->
```rust
/// Where announcements go: a queue, and the consumer of that queue. The queue is in-process
/// here; swapping it for Redis (`nestrs-redis`) changes this module only.
#[module(
    imports = [JobsModule::in_memory_with(jobs_config())],
    providers = [QueuePublisher, Announced, PostPublishedHandler],
    exports = [OutboxPublisherPort, Announced],
)]
pub struct MessagingModule;
```

The blog's test shows the guarantee from outside:

<!-- source: examples/blog-api/tests/outbox.rs -->
```rust
async fn a_post_that_is_refused_is_not_announced() {
    let app = database_app().await;
    let announced = app.resolve::<Announced>();

    // The author does not exist, so nothing is stored and nothing may be announced.
    app.client()
        .post("/posts")
        .bearer("dev-token")
        .json(&json!({ "author_id": "6c1f4a2e-3f3d-4c43-9d44-7a7c5a2f0001", "title": "Ghost", "body": "Post" }))
        .send()
        .await
        .assert_status(StatusCode::BAD_REQUEST);

    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(announced.all().is_empty());
}
```

## Traces

`nestrs-otel` turns the framework's `tracing` spans into OpenTelemetry traces. HTTP requests get a server span named after
the route *template* (`GET /posts/{id}`), a queue handler's span is a child of the request that enqueued the message, and
failures mark the span as an error. The service enables it with one environment variable:

<!-- source: examples/blog-api/src/main.rs -->
```rust
fn start_telemetry() -> Option<Telemetry> {
    let endpoint = std::env::var("BLOG_OTLP_ENDPOINT")
        .ok()
        .filter(|value| !value.is_empty())?;
    let config = OtelConfig::new("blog-api")
        .with_version(env!("CARGO_PKG_VERSION"))
        .with_endpoint(endpoint);
    let telemetry = match Telemetry::init(&config) {
        Ok(telemetry) => telemetry,
        Err(error) => {
            eprintln!("telemetry disabled: {error}");
            return None;
        }
    };
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,tower_http=debug"));
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .with(telemetry.layer())
        .init();
    Some(telemetry)
}
```

Without `BLOG_OTLP_ENDPOINT` the service logs exactly as before. Known gap: an outbox row does not carry trace context, so the
relay's `outbox.publish` span starts a new trace; from the queue onwards the trace is linked.

**SOLID here.** The use case depends on `PostAnnouncements`, not on an outbox or a queue (*dependency inversion*); a
different delivery mechanism is a new adapter (*open/closed*). Jobs and queue handlers emit plain `tracing` spans and know
nothing of OpenTelemetry, which lives in one adapter crate (*single responsibility*).
