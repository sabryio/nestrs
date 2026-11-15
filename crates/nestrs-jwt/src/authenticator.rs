//! The JWT [`Authenticator`].

use std::sync::Arc;
use std::time::Duration;

use jsonwebtoken::errors::ErrorKind;
use jsonwebtoken::{Algorithm, Validation, decode, decode_header};
use nestrs_core::{BoxFuture, Provider};
use nestrs_http::{AuthError, Authenticator, Principal};
use serde_json::Value;

use crate::claims::ClaimMapping;
use crate::keys::{KeyError, KeySource};

/// What a token must satisfy.
///
/// There is deliberately **no `Default`**: you must name the accepted algorithms, so
/// a token cannot pick its own (the classic algorithm-confusion attack).
#[derive(Debug, Clone)]
pub struct JwtConfig {
    algorithms: Vec<Algorithm>,
    issuers: Vec<String>,
    audiences: Vec<String>,
    leeway: Duration,
    claims: ClaimMapping,
}

impl JwtConfig {
    /// Accepts tokens signed with any of `algorithms`.
    ///
    /// # Panics
    /// If `algorithms` is empty: nothing could ever verify.
    pub fn new(algorithms: impl IntoIterator<Item = Algorithm>) -> Self {
        let algorithms: Vec<_> = algorithms.into_iter().collect();
        assert!(
            !algorithms.is_empty(),
            "JwtConfig needs at least one accepted algorithm"
        );
        Self {
            algorithms,
            issuers: Vec::new(),
            audiences: Vec::new(),
            leeway: Duration::from_secs(30),
            claims: ClaimMapping::default(),
        }
    }

    /// HMAC with SHA-256 (shared secret).
    pub fn hs256() -> Self {
        Self::new([Algorithm::HS256])
    }

    /// RSA with SHA-256.
    pub fn rs256() -> Self {
        Self::new([Algorithm::RS256])
    }

    /// ECDSA P-256 with SHA-256.
    pub fn es256() -> Self {
        Self::new([Algorithm::ES256])
    }

    /// Ed25519.
    pub fn eddsa() -> Self {
        Self::new([Algorithm::EdDSA])
    }

    /// Requires the `iss` claim to be one of these. Set it: without it any issuer
    /// that shares your key (or any holder of a token signed by it) is accepted.
    pub fn issuer(mut self, issuers: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.issuers = issuers.into_iter().map(Into::into).collect();
        self
    }

    /// Requires the `aud` claim to contain one of these (this service's name).
    pub fn audience(mut self, audiences: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.audiences = audiences.into_iter().map(Into::into).collect();
        self
    }

    /// Clock-skew tolerance for `exp` and `nbf`. Default 30 s.
    pub fn leeway(mut self, leeway: Duration) -> Self {
        self.leeway = leeway;
        self
    }

    /// Which claims hold roles and permissions.
    pub fn claims(mut self, claims: ClaimMapping) -> Self {
        self.claims = claims;
        self
    }
}

/// Verifies JWT bearer tokens: signature, algorithm, `exp`/`nbf`, issuer, audience,
/// then maps claims to a [`Principal`].
///
/// ```ignore
/// #[provider]
/// fn authenticator(config: Arc<AppConfig>) -> AuthenticatorPort {
///     let keys = StaticKeys::hmac(&config.jwt_secret);
///     Arc::new(JwtAuthenticator::new(Arc::new(keys), JwtConfig::hs256().issuer(["https://id.example.com"]).audience(["blog-api"])))
/// }
/// ```
///
/// Verdicts: a bad signature, wrong algorithm, issuer, audience or shape is
/// [`AuthError::Invalid`]; an elapsed `exp` is [`AuthError::Expired`]; a key source
/// outage is [`AuthError::Unavailable`] (the request is refused, never waved through).
pub struct JwtAuthenticator {
    keys: Arc<dyn KeySource>,
    config: JwtConfig,
}

impl JwtAuthenticator {
    /// An authenticator verifying with keys from `keys` under `config`.
    pub fn new(keys: Arc<dyn KeySource>, config: JwtConfig) -> Self {
        Self { keys, config }
    }

    /// The same, as a provider registering it as the app's [`Authenticator`].
    pub fn provider(keys: Arc<dyn KeySource>, config: JwtConfig) -> Provider {
        Provider::instance::<dyn Authenticator>(Arc::new(Self::new(keys, config)))
    }

    fn validation(&self, algorithm: Algorithm) -> Validation {
        let mut validation = Validation::new(algorithm);
        validation.leeway = self.config.leeway.as_secs();
        validation.set_required_spec_claims(&["exp", "sub"]);
        validation.validate_nbf = true;
        if self.config.issuers.is_empty() {
            validation.iss = None;
        } else {
            validation.set_issuer(&self.config.issuers);
        }
        if self.config.audiences.is_empty() {
            validation.validate_aud = false;
        } else {
            validation.set_audience(&self.config.audiences);
        }
        validation
    }

    async fn verify(&self, token: &str) -> Result<Principal, AuthError> {
        let header = decode_header(token).map_err(|_| AuthError::Invalid)?;
        // The token may not choose its own algorithm: it must be one we allow.
        if !self.config.algorithms.contains(&header.alg) {
            return Err(AuthError::Invalid);
        }
        let key = self
            .keys
            .key(header.kid.as_deref())
            .await
            .map_err(|error| match error {
                KeyError::Unknown => AuthError::Invalid,
                KeyError::Unavailable(reason) => AuthError::Unavailable(reason),
            })?;
        let data = decode::<Value>(token, &key, &self.validation(header.alg)).map_err(|error| match error.kind() {
            ErrorKind::ExpiredSignature => AuthError::Expired,
            _ => AuthError::Invalid,
        })?;
        let subject = data
            .claims
            .get("sub")
            .and_then(Value::as_str)
            .filter(|subject| !subject.is_empty())
            .ok_or(AuthError::Invalid)?;
        Ok(self.config.claims.principal(subject, &data.claims))
    }
}

impl std::fmt::Debug for JwtAuthenticator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JwtAuthenticator")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl Authenticator for JwtAuthenticator {
    fn authenticate<'a>(&'a self, token: &'a str) -> BoxFuture<'a, Result<Principal, AuthError>> {
        Box::pin(self.verify(token))
    }
}
