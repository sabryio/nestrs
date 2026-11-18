//! Where verification keys come from.

use std::collections::HashMap;

use jsonwebtoken::DecodingKey;
use jsonwebtoken::jwk::JwkSet;
use purlin_core::BoxFuture;

/// Why a key could not be produced.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeyError {
    /// No key has this id (or there is no id and no default key): the token is not ours.
    #[error("unknown signing key")]
    Unknown,
    /// The key source could not answer (a JWKS endpoint is down).
    #[error("key source unavailable: {0}")]
    Unavailable(String),
}

/// A malformed key or key set given at start-up.
#[derive(Debug, thiserror::Error)]
#[error("invalid key material: {0}")]
pub struct KeyMaterialError(String);

/// Port: finds the key that verifies a token, by the `kid` in its header.
///
/// [`StaticKeys`] covers shared secrets, PEM files and a JWKS document you already
/// hold. For a JWKS *endpoint* that rotates keys, implement this over an HTTP client
/// with its own cache, then hand it to the authenticator.
///
/// Contract: `Unknown` is a verdict about the token (it names a key we do not trust);
/// `Unavailable` means "could not tell", which makes authentication fail closed.
pub trait KeySource: Send + Sync + 'static {
    /// The key for `kid`; `None` when the token header has no `kid`.
    fn key<'a>(&'a self, kid: Option<&'a str>) -> BoxFuture<'a, Result<DecodingKey, KeyError>>;
}

/// Keys held in memory.
///
/// One *default* key (used when a token has no `kid`) plus any number of keys by id,
/// which is what overlapping rotation looks like.
#[derive(Default)]
pub struct StaticKeys {
    default: Option<DecodingKey>,
    by_id: HashMap<String, DecodingKey>,
}

impl StaticKeys {
    /// No keys: every token is unknown.
    pub fn new() -> Self {
        Self::default()
    }

    /// A shared secret for HS256/HS384/HS512, used for tokens without a `kid`.
    pub fn hmac(secret: impl AsRef<[u8]>) -> Self {
        Self::new().default_key(DecodingKey::from_secret(secret.as_ref()))
    }

    /// A public key in PEM form (RSA, EC or Ed25519), used for tokens without a `kid`.
    pub fn pem(public_key: impl AsRef<[u8]>, family: KeyFamily) -> Result<Self, KeyMaterialError> {
        Ok(Self::new().default_key(decoding_key(public_key.as_ref(), family)?))
    }

    /// A JWKS document (`{"keys": [..]}`); each key is addressed by its `kid`.
    pub fn jwks(json: &str) -> Result<Self, KeyMaterialError> {
        let set: JwkSet = serde_json::from_str(json).map_err(|error| KeyMaterialError(error.to_string()))?;
        let mut keys = Self::new();
        for jwk in &set.keys {
            let key = DecodingKey::from_jwk(jwk).map_err(|error| KeyMaterialError(error.to_string()))?;
            match &jwk.common.key_id {
                Some(kid) => keys = keys.key(kid.clone(), key),
                None if set.keys.len() == 1 => keys = keys.default_key(key),
                None => return Err(KeyMaterialError("a key in a multi-key set has no `kid`".into())),
            }
        }
        Ok(keys)
    }

    /// Adds a key addressed by `kid`.
    pub fn key(mut self, kid: impl Into<String>, key: DecodingKey) -> Self {
        self.by_id.insert(kid.into(), key);
        self
    }

    /// Sets the key used when a token carries no `kid`.
    pub fn default_key(mut self, key: DecodingKey) -> Self {
        self.default = Some(key);
        self
    }
}

impl std::fmt::Debug for StaticKeys {
    // Key material never appears in logs.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StaticKeys")
            .field("default", &self.default.is_some())
            .field("ids", &self.by_id.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl KeySource for StaticKeys {
    fn key<'a>(&'a self, kid: Option<&'a str>) -> BoxFuture<'a, Result<DecodingKey, KeyError>> {
        Box::pin(async move {
            let key = match kid {
                Some(kid) => self.by_id.get(kid),
                None => self.default.as_ref(),
            };
            key.cloned().ok_or(KeyError::Unknown)
        })
    }
}

/// The kind of public key in a PEM file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyFamily {
    /// RSA (RS256, PS256, ..).
    Rsa,
    /// Elliptic curve (ES256, ES384).
    Ec,
    /// Ed25519 (EdDSA).
    Ed25519,
}

fn decoding_key(pem: &[u8], family: KeyFamily) -> Result<DecodingKey, KeyMaterialError> {
    match family {
        KeyFamily::Rsa => DecodingKey::from_rsa_pem(pem),
        KeyFamily::Ec => DecodingKey::from_ec_pem(pem),
        KeyFamily::Ed25519 => DecodingKey::from_ed_pem(pem),
    }
    .map_err(|error| KeyMaterialError(error.to_string()))
}
