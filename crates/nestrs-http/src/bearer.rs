//! Bearer-token authentication: the [`Authenticator`] port and the [`BearerAuth`] guard.
//!
//! ```ignore
//! // 1. An adapter turns a token into a Principal (static table, JWT, introspection ..).
//! #[injectable(provides = AuthenticatorPort)]
//! struct JwtAuthenticator { .. }
//!
//! // 2. The module provides the guard and exports it.
//! #[module(providers = [JwtAuthenticator, BearerAuth], exports = [BearerAuth])]
//! struct AuthModule;
//!
//! // 3. Controllers use it, and add roles/permissions on top.
//! #[controller("/posts")]
//! #[guard(BearerAuth)]
//! impl PostsController { #[post] #[roles("author")] async fn create(..) {..} }
//! ```
//!
//! ## Behaviour
//!
//! * Reads `Authorization: Bearer <token>`; the scheme is case-insensitive. A missing
//!   or malformed header, an unknown token and an expired token are all **401** (with
//!   `WWW-Authenticate: Bearer`); the message says which, the body never echoes the token.
//! * If the [`Authenticator`] itself fails ([`AuthError::Unavailable`]) the request is
//!   refused with **503**: authentication *fails closed*, it never lets a request through.
//! * On success the [`Principal`] is stored in the request extensions, ready for
//!   `#[roles]`, `#[permissions]` and the `Principal` extractor.
//!
//! ## Design
//!
//! The guard only parses the header and maps outcomes to HTTP (SRP). *How* a token is
//! verified is the port's business, so swapping static tokens for JWT is a new adapter
//! and one `provides = ..` (OCP, DIP).

use std::sync::Arc;

use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use nestrs_core::{BoxFuture, Container, DiError, Injectable, Provider, Token};

use crate::{Guard, HttpError, Principal};

/// Why a token was not accepted.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AuthError {
    /// Unknown, malformed or forged: the caller should not retry with the same token.
    #[error("invalid token")]
    Invalid,
    /// Was valid once; the caller should obtain a new one.
    #[error("token expired")]
    Expired,
    /// The verifier could not decide (key server down, introspection timed out).
    #[error("authentication service unavailable: {0}")]
    Unavailable(String),
}

/// Port: verifies a bearer token and says who it belongs to.
///
/// Contract: `Err(Invalid | Expired)` is a verdict about the *token*;
/// `Err(Unavailable)` means "I could not tell", and the guard refuses the request
/// rather than guessing. Implementations must not log or return the token itself.
pub trait Authenticator: Send + Sync + 'static {
    /// Verifies `token` (the part after `Bearer `).
    fn authenticate<'a>(&'a self, token: &'a str) -> BoxFuture<'a, Result<Principal, AuthError>>;
}

/// Shared handle to the registered [`Authenticator`].
pub type AuthenticatorPort = Arc<dyn Authenticator>;

/// Guard: requires a valid bearer token.
pub struct BearerAuth {
    authenticator: AuthenticatorPort,
}

impl BearerAuth {
    /// A guard backed by `authenticator`; normally the container builds it.
    pub fn new(authenticator: AuthenticatorPort) -> Self {
        Self { authenticator }
    }
}

impl Injectable for BearerAuth {
    fn dependencies() -> Vec<Token> {
        vec![Token::of::<dyn Authenticator>()]
    }

    fn build(container: &Container) -> impl std::future::Future<Output = Result<Self, DiError>> + Send {
        let authenticator = container.resolve::<dyn Authenticator>();
        async move { Ok(Self::new(authenticator?)) }
    }
}

/// The token from the request's single `Authorization: Bearer ..` header.
fn bearer_token(parts: &Parts) -> Result<&str, HttpError> {
    let mut values = parts.headers.get_all(AUTHORIZATION).iter();
    let (Some(value), None) = (values.next(), values.next()) else {
        return Err(HttpError::unauthorized(
            match parts.headers.contains_key(AUTHORIZATION) {
                true => "send exactly one Authorization header",
                false => "missing bearer token",
            },
        ));
    };
    let value = value
        .to_str()
        .map_err(|_| HttpError::unauthorized("the Authorization header is not valid text"))?;
    let (scheme, token) = value.split_once(' ').unwrap_or((value, ""));
    let token = token.trim();
    if !scheme.eq_ignore_ascii_case("bearer") || token.is_empty() {
        return Err(HttpError::unauthorized("expected `Authorization: Bearer <token>`"));
    }
    Ok(token)
}

/// Authenticates the request's bearer token and stores the principal.
async fn authenticate(authenticator: &dyn Authenticator, parts: &mut Parts) -> Result<(), HttpError> {
    let token = bearer_token(parts)?;
    match authenticator.authenticate(token).await {
        Ok(principal) => {
            parts.extensions.insert(principal);
            Ok(())
        }
        Err(error @ (AuthError::Invalid | AuthError::Expired)) => Err(HttpError::unauthorized(error.to_string())),
        Err(AuthError::Unavailable(reason)) => {
            tracing::error!(%reason, "authenticator unavailable; refusing the request");
            Err(HttpError::unavailable(
                "authentication is temporarily unavailable",
                None,
            ))
        }
    }
}

impl Guard for BearerAuth {
    async fn check(&self, parts: &mut Parts) -> Result<(), HttpError> {
        authenticate(self.authenticator.as_ref(), parts).await
    }
}

/// Guard: identifies the caller *if* they send a bearer token, and lets anonymous
/// requests through.
///
/// For public routes that behave differently for signed-in users (a feed that marks
/// your own posts). Read the result with an `Option<Principal>` parameter.
///
/// A request that sends a token must send a *good* one: a bad, expired or unverifiable
/// token is still 401/503, never silently downgraded to anonymous, so a client with a
/// stale token learns about it instead of quietly losing its identity.
pub struct OptionalBearerAuth {
    authenticator: AuthenticatorPort,
}

impl OptionalBearerAuth {
    /// A guard backed by `authenticator`; normally the container builds it.
    pub fn new(authenticator: AuthenticatorPort) -> Self {
        Self { authenticator }
    }
}

impl Injectable for OptionalBearerAuth {
    fn dependencies() -> Vec<Token> {
        vec![Token::of::<dyn Authenticator>()]
    }

    fn build(container: &Container) -> impl std::future::Future<Output = Result<Self, DiError>> + Send {
        let authenticator = container.resolve::<dyn Authenticator>();
        async move { Ok(Self::new(authenticator?)) }
    }
}

impl Guard for OptionalBearerAuth {
    const ANONYMOUS_OK: bool = true;

    async fn check(&self, parts: &mut Parts) -> Result<(), HttpError> {
        if !parts.headers.contains_key(AUTHORIZATION) {
            return Ok(());
        }
        authenticate(self.authenticator.as_ref(), parts).await
    }
}

/// A fixed table of tokens and the principals they stand for: for development, tests
/// and simple service-to-service keys.
///
/// Lookups compare against every entry without stopping early, so response time does
/// not reveal how much of a token was right.
///
/// ```ignore
/// providers = [StaticTokens::new()
///     .token("s3cret", Principal::new("admin").with_roles(["admin"]))
///     .provider(), BearerAuth]
/// ```
#[derive(Debug, Clone, Default)]
pub struct StaticTokens {
    entries: Vec<(String, Principal)>,
}

impl StaticTokens {
    /// An empty table: every token is rejected.
    pub fn new() -> Self {
        Self::default()
    }

    /// Accepts `token` as `principal`.
    pub fn token(mut self, token: impl Into<String>, principal: Principal) -> Self {
        self.entries.push((token.into(), principal));
        self
    }

    /// The provider registering this table as the app's [`Authenticator`].
    pub fn provider(self) -> Provider {
        Provider::instance::<dyn Authenticator>(Arc::new(self))
    }
}

impl Authenticator for StaticTokens {
    fn authenticate<'a>(&'a self, token: &'a str) -> BoxFuture<'a, Result<Principal, AuthError>> {
        Box::pin(async move {
            let mut found = None;
            for (known, principal) in &self.entries {
                // No early exit: every entry is compared.
                if constant_time_eq(known.as_bytes(), token.as_bytes()) {
                    found = Some(principal);
                }
            }
            found.cloned().ok_or(AuthError::Invalid)
        })
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let mut difference = a.len() ^ b.len();
    for (x, y) in a.iter().zip(b) {
        difference |= usize::from(x ^ y);
    }
    difference == 0
}

#[cfg(test)]
mod tests {
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::response::Response;
    use axum::routing::get;
    use tower::ServiceExt;

    use super::*;
    use crate::guarded;

    struct Scripted(Result<Principal, AuthError>);
    impl Authenticator for Scripted {
        fn authenticate<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Result<Principal, AuthError>> {
            Box::pin(async move { self.0.clone() })
        }
    }

    fn app(authenticator: impl Authenticator) -> Router {
        let guard = Arc::new(BearerAuth::new(Arc::new(authenticator)));
        let me = |principal: Principal| async move { principal.subject().to_owned() };
        Router::new().route("/me", guarded(get(me), guard))
    }

    async fn call(router: &Router, headers: &[(&str, &str)]) -> Response {
        let mut request = Request::get("/me");
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        router
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    fn table() -> StaticTokens {
        StaticTokens::new()
            .token("s3cret", Principal::new("ann").with_roles(["admin"]))
            .token("other", Principal::new("bob"))
    }

    #[tokio::test]
    async fn a_known_token_reaches_the_handler_as_a_principal() {
        let response = call(&app(table()), &[("authorization", "Bearer s3cret")]).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body[..], b"ann");
    }

    #[tokio::test]
    async fn the_scheme_is_case_insensitive_and_extra_spaces_are_tolerated() {
        for value in ["bearer s3cret", "BEARER s3cret", "Bearer   s3cret "] {
            let response = call(&app(table()), &[("authorization", value)]).await;
            assert_eq!(response.status(), StatusCode::OK, "{value:?}");
        }
    }

    #[tokio::test]
    async fn bad_credentials_are_401_with_a_challenge_and_never_echo_the_token() {
        let router = app(table());
        for (headers, expected) in [
            (vec![], "missing bearer token"),
            (
                vec![("authorization", "Basic s3cret")],
                "expected `Authorization: Bearer <token>`",
            ),
            (
                vec![("authorization", "Bearer")],
                "expected `Authorization: Bearer <token>`",
            ),
            (
                vec![("authorization", "Bearer ")],
                "expected `Authorization: Bearer <token>`",
            ),
            (vec![("authorization", "Bearer wrong-token")], "invalid token"),
            (
                vec![("authorization", "Bearer a"), ("authorization", "Bearer b")],
                "send exactly one Authorization header",
            ),
        ] {
            let response = call(&router, &headers).await;
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{headers:?}");
            assert_eq!(response.headers()["www-authenticate"], "Bearer");
            let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
            let text = String::from_utf8(body.to_vec()).unwrap();
            assert!(text.contains(expected), "{headers:?}: {text}");
            assert!(!text.contains("wrong-token"), "the token must not be echoed: {text}");
        }
    }

    fn optional_app(authenticator: impl Authenticator) -> Router {
        let guard = Arc::new(OptionalBearerAuth::new(Arc::new(authenticator)));
        let who = |principal: Option<Principal>| async move {
            principal.map_or_else(|| "anonymous".to_owned(), |p| p.subject().to_owned())
        };
        Router::new().route("/who", guarded(get(who), guard))
    }

    async fn who(router: &Router, headers: &[(&str, &str)]) -> (StatusCode, String) {
        let mut request = Request::get("/who");
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let response = router
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, String::from_utf8_lossy(&body).into_owned())
    }

    #[tokio::test]
    async fn optional_auth_lets_anonymous_through_and_identifies_signed_in_callers() {
        let router = optional_app(table());
        assert_eq!(who(&router, &[]).await, (StatusCode::OK, "anonymous".to_owned()));
        assert_eq!(
            who(&router, &[("authorization", "Bearer s3cret")]).await,
            (StatusCode::OK, "ann".to_owned())
        );
    }

    #[tokio::test]
    async fn optional_auth_never_downgrades_a_bad_token_to_anonymous() {
        let router = optional_app(table());
        for header in ["Bearer wrong", "Basic abc", "Bearer "] {
            let (status, _) = who(&router, &[("authorization", header)]).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{header:?}");
        }
        let outage = optional_app(Scripted(Err(AuthError::Unavailable("down".into()))));
        assert_eq!(
            who(&outage, &[("authorization", "Bearer t")]).await.0,
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            who(&outage, &[]).await.0,
            StatusCode::OK,
            "anonymous callers do not need the authenticator"
        );
    }

    #[tokio::test]
    async fn an_expired_token_says_so() {
        let response = call(
            &app(Scripted(Err(AuthError::Expired))),
            &[("authorization", "Bearer t")],
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert!(String::from_utf8_lossy(&body).contains("token expired"));
    }

    #[tokio::test]
    async fn an_authenticator_outage_fails_closed_with_503() {
        let outage = Scripted(Err(AuthError::Unavailable("jwks timeout".into())));
        let response = call(&app(outage), &[("authorization", "Bearer t")]).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert!(
            !String::from_utf8_lossy(&body).contains("jwks"),
            "internals stay in the log"
        );
    }

    #[test]
    fn constant_time_comparison_is_exact() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert!(!constant_time_eq(b"", b"a"));
        assert!(constant_time_eq(b"", b""));
    }
}
