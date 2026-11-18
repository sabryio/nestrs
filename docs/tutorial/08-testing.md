# 8. Testing

Three levels, each cheap at its level.

1. **Domain and use cases**: plain Rust, in-memory fakes, no framework.
2. **The real application, in process**: `TestApp` boots `AppModule` and drives it over HTTP without opening a port.
3. **Contracts**: one set of expectations run against every adapter of a port.

## Black-box tests

<!-- source: examples/blog-api/tests/api.rs -->
```rust
async fn health_is_up() {
    let body: Value = client()
        .await
        .get("/health/live")
        .send()
        .await
        .assert_status(StatusCode::OK)
        .json();
    assert_eq!(body, json!({ "status": "ok" }));
}
```

The default token is `dev-token`; writes without it get a 401, and the tests say so.

## Replace one piece and keep the rest

`TestApp::builder(AppModule)` lets a test override any provider. Here a repository that already knows one user stands in for
the real one:

<!-- source: examples/blog-api/tests/overrides.rs -->
```rust
/// A repository that already knows one user, and nothing else.
struct SeededRepository {
    user: User,
}

#[async_trait]
impl UserRepository for SeededRepository {
    async fn find(&self, id: Uuid) -> Result<Option<User>, RepositoryError> {
        Ok((id == self.user.id()).then(|| self.user.clone()))
    }
    async fn find_by_email(&self, email: &Email) -> Result<Option<User>, RepositoryError> {
        Ok((email == self.user.email()).then(|| self.user.clone()))
    }
    async fn list(&self, _page: Page) -> Result<Vec<User>, RepositoryError> {
        Ok(vec![self.user.clone()])
    }
    async fn insert(&self, _user: User) -> Result<(), RepositoryError> {
        Ok(())
    }
}
```

and config is overridden the same way:

<!-- source: examples/blog-api/tests/overrides.rs -->
```rust
async fn overriding_the_config_changes_which_token_is_accepted() {
    let app = TestApp::builder(AppModule)
        .override_value(AppConfig {
            api_token: "test-token".to_owned(),
            ..AppConfig::default()
        })
        .start()
        .await;
```

Nothing in the use cases or controllers changed; only the binding did. That is what depending on ports buys.

## Time without sleeping

Libraries in the workspace that deal with time (the outbox relay, queues) use the Tokio clock, so their tests run with
paused time and finish instantly and deterministically. `purlin-otel` is tested with an in-memory exporter, so span
assertions need no collector.

**SOLID here.** Overrides work because every dependency is an abstraction (*dependency inversion*); a fake and a real adapter
are interchangeable (*Liskov*); a test that needs one behaviour overrides one provider instead of rebuilding the graph
(*interface segregation*).
