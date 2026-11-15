//! # nestrs-jwt
//!
//! A JWT adapter for the [`Authenticator`](nestrs_http::Authenticator) port, so
//! `#[guard(BearerAuth)]` accepts signed tokens and `#[roles]` / `#[permissions]` read
//! their claims.
//!
//! ```ignore
//! #[provider]
//! fn authenticator(config: Arc<AppConfig>) -> AuthenticatorPort {
//!     Arc::new(JwtAuthenticator::new(
//!         Arc::new(StaticKeys::hmac(&config.jwt_secret)),
//!         JwtConfig::hs256().issuer(["https://id.example.com"]).audience(["blog-api"]),
//!     ))
//! }
//! ```
//!
//! ## Security defaults
//!
//! * The accepted algorithms are named by you ([`JwtConfig`] has no `Default`); a
//!   token's own `alg` header can only pick *among* them, and `none` is never accepted.
//! * `exp` and `sub` are required. `nbf` is enforced. Set an issuer and an audience.
//! * A key outage fails closed (`503`), not open.
//! * Verification is local and cheap, so it needs no cache; revocation before `exp`
//!   needs a denylist or short lifetimes.
//!
//! ## Design
//!
//! [`JwtAuthenticator`] verifies, [`KeySource`] finds keys, [`ClaimMapping`] maps
//! claims: three reasons to change, three types. A rotating JWKS endpoint is a new
//! `KeySource`; nothing else changes.

#![forbid(unsafe_code)]

mod authenticator;
mod claims;
mod keys;

pub use authenticator::{JwtAuthenticator, JwtConfig};
pub use claims::ClaimMapping;
pub use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey};
pub use keys::{KeyError, KeyFamily, KeyMaterialError, KeySource, StaticKeys};
