use purlin_http::Interceptor;
use purlin_http::axum::extract::Request;
use purlin_http::axum::http::{HeaderName, StatusCode, header};
use purlin_http::axum::middleware::Next;
use purlin_http::axum::response::Response;

use crate::support::error_response;

/// The tenant a request belongs to. Take it as a handler parameter (or `Option<Tenant>`)
/// once [`TenantResolver`] is installed.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Tenant(String);

impl Tenant {
    /// The tenant id: lowercase letters, digits, `-` and `_`, 1 to 63 characters.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn parse(raw: &str) -> Option<Self> {
        let id = raw.trim().to_ascii_lowercase();
        let valid = (1..=63).contains(&id.len())
            && id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_');
        valid.then_some(Self(id))
    }
}

extension_extractor!(Tenant, "TenantResolver");

#[derive(Debug, Clone)]
enum Source {
    Header(HeaderName),
    Subdomain(String),
}

/// Works out which tenant a request is for, for multi-tenant services.
///
/// Two sources: a header ([`header`](Self::header), `x-tenant-id`) or the first label of
/// the `Host` ([`subdomain`](Self::subdomain), `acme.example.com` with base
/// `example.com` gives `acme`). The id is validated and lowercased, then stored for the
/// [`Tenant`] extractor; an id that is present but malformed is `400` (never silently
/// ignored, so a typo cannot fall through to a default tenant). A missing id is `400`
/// with [`required`](Self::required) and otherwise leaves the request anonymous.
///
/// **A header is a claim, not proof.** If tenants must not read each other's data,
/// derive the tenant from the authenticated identity (a JWT claim) and check it matches,
/// or accept the header only from your own gateway.
#[derive(Debug, Clone)]
pub struct TenantResolver {
    source: Source,
    required: bool,
}

impl TenantResolver {
    /// Reads the tenant from `x-tenant-id`.
    pub fn header() -> Self {
        Self {
            source: Source::Header(HeaderName::from_static("x-tenant-id")),
            required: false,
        }
    }

    /// Reads the tenant from the first label of the host under `base_domain`.
    pub fn subdomain(base_domain: &str) -> Self {
        Self {
            source: Source::Subdomain(base_domain.trim_start_matches('.').to_ascii_lowercase()),
            required: false,
        }
    }

    /// Rejects requests with no tenant.
    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }

    fn raw(&self, request: &Request) -> Option<String> {
        match &self.source {
            Source::Header(name) => request.headers().get(name)?.to_str().ok().map(str::to_owned),
            Source::Subdomain(base) => {
                let host = request.headers().get(header::HOST)?.to_str().ok()?;
                let host = host.split(':').next()?.to_ascii_lowercase();
                let prefix = host.strip_suffix(base.as_str())?.strip_suffix('.')?;
                (!prefix.contains('.')).then(|| prefix.to_owned())
            }
        }
    }
}

impl Interceptor for TenantResolver {
    async fn intercept(&self, mut request: Request, next: Next) -> Response {
        match self.raw(&request) {
            Some(raw) => match Tenant::parse(&raw) {
                Some(tenant) => {
                    request.extensions_mut().insert(tenant);
                }
                None => {
                    return error_response(
                        StatusCode::BAD_REQUEST,
                        "invalid_tenant",
                        "the tenant id is not valid",
                        None,
                    );
                }
            },
            None if self.required => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "tenant_required",
                    "this request needs a tenant",
                    None,
                );
            }
            None => {}
        }
        next.run(request).await
    }
}

#[cfg(test)]
mod tests {
    use purlin_http::axum::Router;
    use purlin_http::axum::routing::get as get_route;
    use purlin_http::intercepted;
    use std::sync::Arc;

    use crate::testing::{get, text};

    use super::*;

    fn app(resolver: TenantResolver) -> Router {
        let handler =
            get_route(
                |tenant: Option<Tenant>| async move { tenant.map_or("none".to_owned(), |t| t.as_str().to_owned()) },
            );
        Router::new().route("/", intercepted(handler, Arc::new(resolver)))
    }

    #[tokio::test]
    async fn resolves_from_a_header_and_normalises() {
        let app = app(TenantResolver::header());
        assert_eq!(text(get(&app, "/", &[("x-tenant-id", " Acme ")]).await).await, "acme");
        assert_eq!(text(get(&app, "/", &[]).await).await, "none");
        assert_eq!(
            get(&app, "/", &[("x-tenant-id", "a b")]).await.status(),
            StatusCode::BAD_REQUEST
        );
    }

    #[tokio::test]
    async fn resolves_from_a_subdomain() {
        let app = app(TenantResolver::subdomain("example.com").required());
        assert_eq!(
            text(get(&app, "/", &[("host", "acme.example.com:8080")]).await).await,
            "acme"
        );
        for host in ["example.com", "a.b.example.com", "acme.other.org"] {
            let response = get(&app, "/", &[("host", host)]).await;
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{host}");
        }
    }
}
