use purlin_http::Interceptor;
use purlin_http::axum::extract::Request;
use purlin_http::axum::http::{HeaderValue, header};
use purlin_http::axum::middleware::Next;
use purlin_http::axum::response::Response;

/// Marks a route as deprecated for its clients without breaking them.
///
/// Adds `Deprecation: true`, optionally `Sunset: <HTTP-date>` (RFC 8594, when the route
/// will stop working) and `Link: <url>; rel="successor-version"`.
///
/// ```ignore
/// #[interceptor(Deprecated::new().sunset("Wed, 01 Jul 2027 00:00:00 GMT").successor("/api/v2/posts"))]
/// ```
#[derive(Debug, Clone, Default)]
pub struct Deprecated {
    sunset: Option<HeaderValue>,
    link: Option<HeaderValue>,
}

impl Deprecated {
    /// A deprecation notice with no date or successor.
    pub fn new() -> Self {
        Self::default()
    }

    /// When the route is removed, as an HTTP-date.
    ///
    /// # Panics
    /// If `http_date` is not a valid header value.
    pub fn sunset(mut self, http_date: &str) -> Self {
        self.sunset = Some(HeaderValue::from_str(http_date).expect("a valid HTTP-date"));
        self
    }

    /// The URL that replaces this route.
    ///
    /// # Panics
    /// If `url` cannot be written in a header.
    pub fn successor(mut self, url: &str) -> Self {
        self.link = Some(HeaderValue::from_str(&format!("<{url}>; rel=\"successor-version\"")).expect("a valid URL"));
        self
    }
}

impl Interceptor for Deprecated {
    async fn intercept(&self, request: Request, next: Next) -> Response {
        let mut response = next.run(request).await;
        let headers = response.headers_mut();
        headers.insert("deprecation", HeaderValue::from_static("true"));
        if let Some(sunset) = &self.sunset {
            headers.insert("sunset", sunset.clone());
        }
        if let Some(link) = &self.link {
            headers.append(header::LINK, link.clone());
        }
        response
    }
}

#[cfg(test)]
mod tests {
    use crate::testing::{get, router_with};

    use super::*;

    #[tokio::test]
    async fn announces_deprecation_sunset_and_successor() {
        let app = router_with(
            Deprecated::new()
                .sunset("Wed, 01 Jul 2027 00:00:00 GMT")
                .successor("/v2/x"),
        );
        let response = get(&app, "/", &[]).await;
        assert_eq!(response.headers()["deprecation"], "true");
        assert_eq!(response.headers()["sunset"], "Wed, 01 Jul 2027 00:00:00 GMT");
        assert_eq!(response.headers()["link"], "</v2/x>; rel=\"successor-version\"");
    }
}
