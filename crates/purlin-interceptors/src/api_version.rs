use purlin_http::Interceptor;
use purlin_http::axum::extract::Request;
use purlin_http::axum::http::{HeaderName, HeaderValue, StatusCode};
use purlin_http::axum::middleware::Next;
use purlin_http::axum::response::Response;
use serde_json::json;

use crate::support::error_response;

/// The API version a request asked for. Take it as a handler parameter once
/// [`ApiVersions`] is installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ApiVersion(pub u32);

extension_extractor!(ApiVersion, "ApiVersions");

/// Resolves the API version from the `api-version` header (`2` or `v2`).
///
/// No header means the default version (so old clients keep working when you add a
/// version). A version you do not support is `400`, code `unsupported_api_version`, with
/// the supported list in `details`, rather than silently serving something else. The
/// response echoes the version used in `api-version`.
///
/// Handlers branch on the [`ApiVersion`] extractor, or you mount two controllers and
/// attach `ApiVersions` with a single supported version to each.
#[derive(Debug, Clone)]
pub struct ApiVersions {
    supported: Vec<u32>,
    default: u32,
    header: HeaderName,
}

impl ApiVersions {
    /// Supports `supported`; `default` is used when the client names none.
    ///
    /// # Panics
    /// If `default` is not in `supported`.
    pub fn new(supported: impl IntoIterator<Item = u32>, default: u32) -> Self {
        let supported: Vec<u32> = supported.into_iter().collect();
        assert!(
            supported.contains(&default),
            "the default version must be a supported one"
        );
        Self {
            supported,
            default,
            header: HeaderName::from_static("api-version"),
        }
    }
}

fn parse(raw: &str) -> Option<u32> {
    raw.trim().trim_start_matches(['v', 'V']).parse().ok()
}

impl Interceptor for ApiVersions {
    async fn intercept(&self, mut request: Request, next: Next) -> Response {
        let version = match request.headers().get(&self.header) {
            None => self.default,
            Some(raw) => match raw.to_str().ok().and_then(parse).filter(|v| self.supported.contains(v)) {
                Some(version) => version,
                None => {
                    return error_response(
                        StatusCode::BAD_REQUEST,
                        "unsupported_api_version",
                        "this API version is not supported",
                        Some(json!({ "supported": self.supported })),
                    );
                }
            },
        };
        request.extensions_mut().insert(ApiVersion(version));
        let mut response = next.run(request).await;
        response
            .headers_mut()
            .insert(self.header.clone(), HeaderValue::from(version));
        response
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use purlin_http::axum::Router;
    use purlin_http::axum::routing::get as get_route;
    use purlin_http::intercepted;

    use crate::testing::{get, text};

    use super::*;

    fn app() -> Router {
        let handler = get_route(|version: ApiVersion| async move { format!("v{}", version.0) });
        Router::new().route("/", intercepted(handler, Arc::new(ApiVersions::new([1, 2], 2))))
    }

    #[tokio::test]
    async fn defaults_parses_and_refuses_unknown_versions() {
        let app = app();
        assert_eq!(text(get(&app, "/", &[]).await).await, "v2");
        assert_eq!(text(get(&app, "/", &[("api-version", "v1")]).await).await, "v1");
        let echoed = get(&app, "/", &[("api-version", "2")]).await;
        assert_eq!(echoed.headers()["api-version"], "2");

        for bad in ["3", "x", "-1"] {
            let response = get(&app, "/", &[("api-version", bad)]).await;
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{bad}");
            assert!(text(response).await.contains("\"supported\":[1,2]"));
        }
    }
}
