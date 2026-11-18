use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};
use validator::ValidationErrors;

use crate::filter::CaughtError;

/// The one error type HTTP handlers and guards return.
///
/// It renders as `{"error": {"code": "..", "message": "..", "details": ..}}` with a
/// matching status code, so clients see a single, predictable shape. Domain errors
/// stay free of HTTP: services return their own enums and the controller layer maps
/// them with `From<DomainError> for HttpError`.
#[derive(Debug, Clone, thiserror::Error)]
pub enum HttpError {
    /// 400: the request was malformed.
    #[error("{0}")]
    BadRequest(String),
    /// 401: credentials missing or invalid.
    #[error("{0}")]
    Unauthorized(String),
    /// 403: authenticated, but not allowed.
    #[error("{0}")]
    Forbidden(String),
    /// 404: the resource does not exist.
    #[error("{0}")]
    NotFound(String),
    /// 409: the request conflicts with current state.
    #[error("{0}")]
    Conflict(String),
    /// 422: the body parsed but failed validation rules; carries per-field details.
    #[error("request validation failed")]
    Validation(Value),
    /// 429: the caller exceeded a rate limit; `retry_after` says when to come back.
    #[error("rate limit exceeded; retry after {}s", .retry_after.as_secs().max(1))]
    TooManyRequests {
        /// How long until a retry can succeed.
        retry_after: Duration,
    },
    /// 503: the service cannot serve right now (not ready, draining). `details` is
    /// optional structured context, such as which dependency is down.
    #[error("{message}")]
    Unavailable {
        /// What is wrong, for people.
        message: String,
        /// Structured context, rendered under `error.details`.
        details: Option<Value>,
    },
    /// 504: the handler did not finish in time (see the `Timeout` interceptor).
    #[error("the request took too long to complete")]
    Timeout,
    /// 500: something broke. The detail is logged, never sent to the client.
    #[error("internal server error")]
    Internal(String),
}

impl HttpError {
    /// The status code this error renders with.
    pub fn status(&self) -> StatusCode {
        match self {
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            Self::Forbidden(_) => StatusCode::FORBIDDEN,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::Validation(_) => StatusCode::UNPROCESSABLE_ENTITY,
            Self::TooManyRequests { .. } => StatusCode::TOO_MANY_REQUESTS,
            Self::Unavailable { .. } => StatusCode::SERVICE_UNAVAILABLE,
            Self::Timeout => StatusCode::GATEWAY_TIMEOUT,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// Stable machine-readable code for clients.
    pub fn code(&self) -> &'static str {
        match self {
            Self::BadRequest(_) => "bad_request",
            Self::Unauthorized(_) => "unauthorized",
            Self::Forbidden(_) => "forbidden",
            Self::NotFound(_) => "not_found",
            Self::Conflict(_) => "conflict",
            Self::Validation(_) => "validation_failed",
            Self::TooManyRequests { .. } => "rate_limited",
            Self::Unavailable { .. } => "unavailable",
            Self::Timeout => "timeout",
            Self::Internal(_) => "internal",
        }
    }

    /// Shorthand for [`HttpError::BadRequest`].
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::BadRequest(message.into())
    }

    /// Shorthand for [`HttpError::Unauthorized`].
    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::Unauthorized(message.into())
    }

    /// Shorthand for [`HttpError::Forbidden`].
    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::Forbidden(message.into())
    }

    /// Shorthand for [`HttpError::NotFound`].
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::NotFound(message.into())
    }

    /// Shorthand for [`HttpError::Conflict`].
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::Conflict(message.into())
    }

    /// Shorthand for [`HttpError::TooManyRequests`].
    pub fn too_many_requests(retry_after: Duration) -> Self {
        Self::TooManyRequests { retry_after }
    }

    /// Shorthand for [`HttpError::Unavailable`].
    pub fn unavailable(message: impl Into<String>, details: Option<Value>) -> Self {
        Self::Unavailable {
            message: message.into(),
            details,
        }
    }

    /// Shorthand for [`HttpError::Internal`]; `detail` is logged, not exposed.
    pub fn internal(detail: impl Into<String>) -> Self {
        Self::Internal(detail.into())
    }
}

impl From<ValidationErrors> for HttpError {
    fn from(errors: ValidationErrors) -> Self {
        let mut fields = Map::new();
        for (field, failures) in errors.field_errors() {
            let failures: Vec<Value> = failures
                .iter()
                .map(|failure| json!({ "code": failure.code, "message": failure.message }))
                .collect();
            fields.insert(field.to_string(), Value::Array(failures));
        }
        Self::Validation(Value::Object(fields))
    }
}

impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        if let Self::Internal(detail) = &self {
            tracing::error!(%detail, "internal server error");
        }

        let mut error = json!({ "code": self.code(), "message": self.to_string() });
        if let Self::Validation(details) = &self {
            error["details"] = details.clone();
        }
        if let Self::Unavailable {
            details: Some(details), ..
        } = &self
        {
            error["details"] = details.clone();
        }
        // Whole seconds, rounded up, so "retry after" never undershoots.
        let retry_after_seconds = match &self {
            Self::TooManyRequests { retry_after } => {
                Some(retry_after.as_secs() + u64::from(retry_after.subsec_nanos() > 0))
            }
            _ => None,
        };
        if let Some(seconds) = retry_after_seconds {
            error["details"] = json!({ "retry_after_seconds": seconds });
        }
        let mut response = (self.status(), Json(json!({ "error": error }))).into_response();
        if let Some(seconds) = retry_after_seconds {
            response
                .headers_mut()
                .insert(axum::http::header::RETRY_AFTER, seconds.into());
        }
        // A 401 must say how to authenticate (RFC 9110); the API's scheme is bearer tokens.
        if matches!(self, Self::Unauthorized(_)) {
            response.headers_mut().insert(
                axum::http::header::WWW_AUTHENTICATE,
                axum::http::HeaderValue::from_static("Bearer"),
            );
        }
        // Lets exception filters see the error itself, not just the rendered body.
        response.extensions_mut().insert(CaughtError(Arc::new(self)));
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    async fn body_of(error: HttpError) -> (StatusCode, Value) {
        let response = error.into_response();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn renders_a_uniform_envelope() {
        let (status, body) = body_of(HttpError::not_found("no such user")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(
            body,
            json!({ "error": { "code": "not_found", "message": "no such user" } })
        );
    }

    #[tokio::test]
    async fn internal_errors_do_not_leak_details() {
        let (status, body) = body_of(HttpError::internal("password=hunter2")).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["error"]["message"], "internal server error");
        assert!(!body.to_string().contains("hunter2"));
    }
}
