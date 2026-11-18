use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use purlin_core::BoxFuture;
use purlin_http::axum::extract::Request;
use purlin_http::axum::http::Method;
use purlin_http::axum::middleware::Next;
use purlin_http::axum::response::Response;
use purlin_http::{Interceptor, Principal};

use crate::request_id::RequestIdValue;

/// One audited request: who did what, and how it ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditRecord {
    /// The authenticated subject, or `None` for anonymous callers.
    pub subject: Option<String>,
    /// The request id, when [`RequestId`](crate::RequestId) is installed outside.
    pub request_id: Option<String>,
    /// HTTP method.
    pub method: String,
    /// Path without the query string (which may carry secrets).
    pub path: String,
    /// Response status.
    pub status: u16,
    /// How long the request took.
    pub elapsed: Duration,
}

/// Where audit records go: a table, a log pipeline, an event bus.
pub trait AuditSink: Send + Sync + 'static {
    /// Stores one record. Failures are the sink's to handle (retry, log); the request
    /// is never failed because auditing was.
    fn record(&self, record: AuditRecord) -> BoxFuture<'_, ()>;
}

/// Writes records as structured `tracing` events on the `audit` target.
#[derive(Debug, Default, Clone, Copy)]
pub struct TracingAuditSink;

impl AuditSink for TracingAuditSink {
    fn record(&self, record: AuditRecord) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            tracing::info!(
                target: "audit",
                subject = record.subject.as_deref().unwrap_or("anonymous"),
                request_id = record.request_id.as_deref().unwrap_or(""),
                method = %record.method,
                path = %record.path,
                status = record.status,
                elapsed_ms = record.elapsed.as_millis() as u64,
                "audit"
            );
        })
    }
}

/// Keeps records in memory; for tests and for inspecting behaviour in development.
#[derive(Debug, Default)]
pub struct MemoryAuditSink {
    records: Mutex<Vec<AuditRecord>>,
}

impl MemoryAuditSink {
    /// An empty sink.
    pub fn new() -> Self {
        Self::default()
    }

    /// A copy of everything recorded so far.
    pub fn records(&self) -> Vec<AuditRecord> {
        self.records.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

impl AuditSink for MemoryAuditSink {
    fn record(&self, record: AuditRecord) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.records.lock().unwrap_or_else(PoisonError::into_inner).push(record);
        })
    }
}

/// Records who changed what. By default only unsafe methods (`POST`, `PUT`, `PATCH`,
/// `DELETE`) are audited; [`all_methods`](Self::all_methods) includes reads.
///
/// Install it **inside** the auth guard (as a route or controller interceptor) so the
/// caller is known; outside the guard every record is anonymous. With
/// [`detached`](Self::detached) the sink runs in a background task, so a slow audit store
/// never delays the response (at the price that a crash can lose the last records).
#[derive(Clone)]
pub struct Audit {
    sink: Arc<dyn AuditSink>,
    all_methods: bool,
    detached: bool,
}

impl Audit {
    /// Audits writes to `sink`, waiting for the sink before responding.
    pub fn new(sink: Arc<dyn AuditSink>) -> Self {
        Self {
            sink,
            all_methods: false,
            detached: false,
        }
    }

    /// Audits reads as well.
    pub fn all_methods(mut self) -> Self {
        self.all_methods = true;
        self
    }

    /// Records in a background task instead of before the response.
    pub fn detached(mut self) -> Self {
        self.detached = true;
        self
    }
}

impl Interceptor for Audit {
    async fn intercept(&self, request: Request, next: Next) -> Response {
        let safe = matches!(*request.method(), Method::GET | Method::HEAD | Method::OPTIONS);
        if safe && !self.all_methods {
            return next.run(request).await;
        }
        let subject = request.extensions().get::<Principal>().map(|p| p.subject().to_owned());
        let request_id = request
            .extensions()
            .get::<RequestIdValue>()
            .map(|id| id.0.clone())
            .or_else(|| {
                request
                    .headers()
                    .get("x-request-id")
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned)
            });
        let method = request.method().to_string();
        let path = request.uri().path().to_owned();

        let started = Instant::now();
        let response = next.run(request).await;
        let record = AuditRecord {
            subject,
            request_id,
            method,
            path,
            status: response.status().as_u16(),
            elapsed: started.elapsed(),
        };
        if self.detached {
            let sink = self.sink.clone();
            tokio::spawn(async move { sink.record(record).await });
        } else {
            self.sink.record(record).await;
        }
        response
    }
}

#[cfg(test)]
mod tests {
    use purlin_http::axum::Router;
    use purlin_http::axum::http::StatusCode;
    use purlin_http::axum::routing::{get as get_route, post as post_route};
    use purlin_http::intercepted;

    use crate::testing::{get, post_with};

    use super::*;

    fn app(audit: Audit) -> Router {
        let audit = Arc::new(audit);
        Router::new().route(
            "/",
            intercepted(
                get_route(|| async { "ok" }).merge(post_route(|| async { StatusCode::CREATED })),
                audit,
            ),
        )
    }

    #[tokio::test]
    async fn audits_writes_with_request_id_and_skips_reads_by_default() {
        let sink = Arc::new(MemoryAuditSink::new());
        let app = app(Audit::new(sink.clone()));
        get(&app, "/", &[]).await;
        post_with(&app, "/?token=secret", &[("x-request-id", "r-1")], "").await;

        let records = sink.records();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].method, "POST");
        assert_eq!(records[0].path, "/", "the query string is dropped");
        assert_eq!(records[0].status, 201);
        assert_eq!(records[0].request_id.as_deref(), Some("r-1"));
        assert_eq!(records[0].subject, None);
    }

    #[tokio::test]
    async fn can_include_reads_and_record_in_the_background() {
        let sink = Arc::new(MemoryAuditSink::new());
        let app = app(Audit::new(sink.clone()).all_methods().detached());
        get(&app, "/", &[]).await;
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;
        assert_eq!(sink.records().len(), 1);
    }

    #[tokio::test]
    async fn records_the_authenticated_subject() {
        use purlin_http::axum::middleware::{self, Next};
        let sink = Arc::new(MemoryAuditSink::new());
        let inner = app(Audit::new(sink.clone()));
        let app = inner.layer(middleware::from_fn(|mut request: Request, next: Next| async move {
            request.extensions_mut().insert(Principal::new("alice"));
            next.run(request).await
        }));
        post_with(&app, "/", &[], "").await;
        assert_eq!(sink.records()[0].subject.as_deref(), Some("alice"));
    }
}
