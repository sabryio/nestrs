use nestrs_http::Interceptor;
use nestrs_http::axum::extract::Request;
use nestrs_http::axum::http::{HeaderName, HeaderValue};
use nestrs_http::axum::middleware::Next;
use nestrs_http::axum::response::Response;

/// Adds the standard hardening headers to every response.
///
/// Defaults suit a JSON API: `x-content-type-options: nosniff`, `x-frame-options: DENY`,
/// `referrer-policy: no-referrer`, `cross-origin-opener-policy: same-origin`,
/// `strict-transport-security: max-age=31536000; includeSubDomains` and
/// `content-security-policy: default-src 'none'; frame-ancestors 'none'`.
///
/// A header the handler already set is never overwritten, so one route can opt out.
/// The strict CSP blocks Swagger UI's scripts: relax it with [`csp`](Self::csp) or
/// install this interceptor on the API routes only.
#[derive(Debug, Clone)]
pub struct SecurityHeaders {
    headers: Vec<(HeaderName, HeaderValue)>,
}

impl SecurityHeaders {
    /// Sets (or replaces) one header.
    ///
    /// # Panics
    /// If `value` is not a valid header value; this is a startup typo, not a runtime case.
    pub fn set(mut self, name: &'static str, value: &str) -> Self {
        let name = HeaderName::from_static(name);
        let value = HeaderValue::from_str(value).expect("a valid header value");
        self.headers.retain(|(existing, _)| *existing != name);
        self.headers.push((name, value));
        self
    }

    /// Replaces the `content-security-policy`.
    pub fn csp(self, policy: &str) -> Self {
        self.set("content-security-policy", policy)
    }

    /// Drops `strict-transport-security`, for services not served over HTTPS.
    pub fn without_hsts(mut self) -> Self {
        self.headers
            .retain(|(name, _)| name.as_str() != "strict-transport-security");
        self
    }
}

impl Default for SecurityHeaders {
    fn default() -> Self {
        Self { headers: Vec::new() }
            .set("x-content-type-options", "nosniff")
            .set("x-frame-options", "DENY")
            .set("referrer-policy", "no-referrer")
            .set("cross-origin-opener-policy", "same-origin")
            .set("strict-transport-security", "max-age=31536000; includeSubDomains")
            .set("content-security-policy", "default-src 'none'; frame-ancestors 'none'")
    }
}

impl Interceptor for SecurityHeaders {
    async fn intercept(&self, request: Request, next: Next) -> Response {
        let mut response = next.run(request).await;
        for (name, value) in &self.headers {
            if !response.headers().contains_key(name) {
                response.headers_mut().insert(name.clone(), value.clone());
            }
        }
        response
    }
}

#[cfg(test)]
mod tests {
    use crate::testing::{get, router_with};

    use super::*;

    #[tokio::test]
    async fn adds_defaults_and_honours_overrides() {
        let app = router_with(SecurityHeaders::default().csp("default-src 'self'").without_hsts());
        let response = get(&app, "/", &[]).await;
        assert_eq!(response.headers()["x-content-type-options"], "nosniff");
        assert_eq!(response.headers()["content-security-policy"], "default-src 'self'");
        assert!(!response.headers().contains_key("strict-transport-security"));
    }

    #[tokio::test]
    async fn never_overwrites_what_the_handler_set() {
        let app = crate::testing::router_with_handler(SecurityHeaders::default(), || async {
            ([("x-frame-options", "SAMEORIGIN")], "ok")
        });
        let response = get(&app, "/", &[]).await;
        assert_eq!(response.headers()["x-frame-options"], "SAMEORIGIN");
    }
}
