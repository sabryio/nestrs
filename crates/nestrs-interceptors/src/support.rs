//! Pieces shared by the interceptors: the error envelope, buffered responses, hashing.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};

use nestrs_http::axum::Json;
use nestrs_http::axum::body::{Body, Bytes, to_bytes};
use nestrs_http::axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use nestrs_http::axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// A response in the same `{"error": {"code", "message"}}` envelope `HttpError` uses,
/// for statuses `HttpError` has no variant for (413, 415, 422 with a custom code).
pub(crate) fn error_response(status: StatusCode, code: &str, message: &str, details: Option<Value>) -> Response {
    let mut error = json!({ "code": code, "message": message });
    if let Some(details) = details {
        error["details"] = details;
    }
    (status, Json(json!({ "error": error }))).into_response()
}

/// Adds `Retry-After` (whole seconds, at least 1) to a response.
pub(crate) fn with_retry_after(mut response: Response, seconds: u64) -> Response {
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from(seconds.max(1)));
    response
}

/// A fully buffered response that can be stored, replayed and shared.
#[derive(Debug, Clone)]
pub struct Snapshot {
    /// Status code.
    pub status: StatusCode,
    /// Headers, including `content-length`.
    pub headers: HeaderMap,
    /// The whole body.
    pub body: Bytes,
}

impl Snapshot {
    /// Rebuilds a response from the snapshot. It can be called any number of times.
    pub fn to_response(&self) -> Response {
        let mut response = Response::new(Body::from(self.body.clone()));
        *response.status_mut() = self.status;
        *response.headers_mut() = self.headers.clone();
        response
    }

    /// Buffers `response` when that is safe: its length is known and at most `limit`,
    /// and it is not a stream. Returns the response to send (rebuilt when it was
    /// buffered, untouched otherwise) and the snapshot when one was taken.
    ///
    /// Streams (Server-Sent Events, chunked bodies without a length) are never
    /// buffered; they pass through unchanged and give no snapshot.
    pub async fn capture(response: Response, limit: usize) -> (Response, Option<Snapshot>) {
        // The length is either declared in a header or known from the body itself (axum
        // leaves `content-length` to the server, so a handler's `String` has no header yet).
        let declared = response
            .headers()
            .get(header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<usize>().ok())
            .or_else(|| {
                http_body::Body::size_hint(response.body())
                    .exact()
                    .and_then(|n| usize::try_from(n).ok())
            });
        let is_stream = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/event-stream"));
        if is_stream || declared.is_none_or(|length| length > limit) {
            return (response, None);
        }
        let (parts, body) = response.into_parts();
        match to_bytes(body, limit).await {
            Ok(body) => {
                let mut headers = parts.headers;
                headers.insert(header::CONTENT_LENGTH, HeaderValue::from(body.len()));
                let snapshot = Snapshot {
                    status: parts.status,
                    headers,
                    body,
                };
                (snapshot.to_response(), Some(snapshot))
            }
            Err(error) => {
                tracing::error!(%error, "could not buffer a response that declared a length");
                (
                    error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "internal",
                        "internal server error",
                        None,
                    ),
                    None,
                )
            }
        }
    }
}

/// The first `bytes` bytes of the SHA-256 of `parts`, as lowercase hex.
pub(crate) fn digest_hex(parts: &[&[u8]], bytes: usize) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part);
    }
    hasher
        .finalize()
        .iter()
        .take(bytes)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A random `u64` without a dependency: `RandomState` is seeded from the OS.
pub(crate) fn random_u64() -> u64 {
    RandomState::new().build_hasher().finish()
}
