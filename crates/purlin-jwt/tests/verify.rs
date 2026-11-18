//! Verification behaviour against real signed tokens.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use jsonwebtoken::{EncodingKey, Header, encode};
use purlin_http::{AuthError, Authenticator};
use purlin_jwt::{
    Algorithm, ClaimMapping, DecodingKey, JwtAuthenticator, JwtConfig, KeyError, KeyFamily, KeySource, StaticKeys,
};
use serde_json::{Value, json};

const SECRET: &[u8] = b"test-secret-0123456789-0123456789";

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

fn token_with(header: Header, claims: Value, secret: &[u8]) -> String {
    encode(&header, &claims, &EncodingKey::from_secret(secret)).unwrap()
}

fn token(claims: Value) -> String {
    token_with(Header::new(Algorithm::HS256), claims, SECRET)
}

fn valid() -> Value {
    json!({ "sub": "ann", "exp": now() + 3600, "iss": "https://id.test", "aud": "blog-api",
            "roles": ["author"], "scope": "posts:create posts:read" })
}

fn authenticator() -> JwtAuthenticator {
    let config = JwtConfig::hs256().issuer(["https://id.test"]).audience(["blog-api"]);
    JwtAuthenticator::new(Arc::new(StaticKeys::hmac(SECRET)), config)
}

#[tokio::test]
async fn a_valid_token_becomes_a_principal_with_roles_and_permissions() {
    let principal = authenticator().authenticate(&token(valid())).await.unwrap();
    assert_eq!(principal.subject(), "ann");
    assert!(principal.has_role("author"));
    assert!(principal.has_permission("posts:create") && principal.has_permission("posts:read"));
    assert!(!principal.has_role("admin"));
}

#[tokio::test]
async fn an_expired_token_is_expired_not_just_invalid() {
    let mut claims = valid();
    claims["exp"] = json!(now() - 3600);
    assert_eq!(
        authenticator().authenticate(&token(claims)).await,
        Err(AuthError::Expired)
    );
}

#[tokio::test]
async fn leeway_forgives_small_clock_skew_only() {
    let mut claims = valid();
    claims["exp"] = json!(now() - 5);
    assert!(
        authenticator().authenticate(&token(claims.clone())).await.is_ok(),
        "5 s late is within the 30 s leeway"
    );
    claims["exp"] = json!(now() - 120);
    assert_eq!(
        authenticator().authenticate(&token(claims)).await,
        Err(AuthError::Expired)
    );
}

#[tokio::test]
async fn forged_wrong_or_misshapen_tokens_are_invalid() {
    let auth = authenticator();
    let forged = token_with(
        Header::new(Algorithm::HS256),
        valid(),
        b"another-secret-another-secret-1234",
    );
    let mut no_sub = valid();
    no_sub.as_object_mut().unwrap().remove("sub");
    let mut no_exp = valid();
    no_exp.as_object_mut().unwrap().remove("exp");
    let mut not_yet = valid();
    not_yet["nbf"] = json!(now() + 3600);
    let mut other_issuer = valid();
    other_issuer["iss"] = json!("https://evil.test");
    let mut other_audience = valid();
    other_audience["aud"] = json!("someone-else");

    for (name, bad) in [
        ("wrong secret", forged),
        ("no subject", token(no_sub)),
        ("no expiry", token(no_exp)),
        ("not valid yet", token(not_yet)),
        ("other issuer", token(other_issuer)),
        ("other audience", token(other_audience)),
        ("garbage", "not.a.jwt".to_owned()),
        ("empty", String::new()),
    ] {
        assert_eq!(auth.authenticate(&bad).await, Err(AuthError::Invalid), "{name}");
    }
}

#[tokio::test]
async fn the_token_cannot_choose_its_own_algorithm() {
    // Signed correctly with HS384, but only HS256 is accepted.
    let hs384 = token_with(Header::new(Algorithm::HS384), valid(), SECRET);
    assert_eq!(authenticator().authenticate(&hs384).await, Err(AuthError::Invalid));

    // `alg: none` (an unsigned token) is never accepted.
    let unsigned = format!(
        "{}.{}.",
        base64url(br#"{"alg":"none","typ":"JWT"}"#),
        base64url(valid().to_string().as_bytes())
    );
    assert_eq!(authenticator().authenticate(&unsigned).await, Err(AuthError::Invalid));
}

fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |acc, (i, b)| acc | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..=chunk.len() {
            out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
    }
    out
}

#[tokio::test]
async fn issuer_and_audience_are_optional_checks() {
    let lenient = JwtAuthenticator::new(Arc::new(StaticKeys::hmac(SECRET)), JwtConfig::hs256());
    let mut claims = valid();
    claims["iss"] = json!("anyone");
    claims["aud"] = json!("anything");
    assert!(lenient.authenticate(&token(claims)).await.is_ok());
}

#[tokio::test]
async fn keys_are_selected_by_kid_which_makes_rotation_overlap_possible() {
    let keys = StaticKeys::new()
        .key("2025", DecodingKey::from_secret(b"old-old-old-old-old-old-old-old-12"))
        .key("2026", DecodingKey::from_secret(SECRET));
    let auth = JwtAuthenticator::new(Arc::new(keys), JwtConfig::hs256());

    let mut header = Header::new(Algorithm::HS256);
    header.kid = Some("2026".into());
    assert!(
        auth.authenticate(&token_with(header.clone(), valid(), SECRET))
            .await
            .is_ok()
    );

    header.kid = Some("2025".into());
    assert!(
        auth.authenticate(&token_with(
            header.clone(),
            valid(),
            b"old-old-old-old-old-old-old-old-12"
        ))
        .await
        .is_ok(),
        "the previous key still verifies during the overlap"
    );

    header.kid = Some("1999".into());
    assert_eq!(
        auth.authenticate(&token_with(header, valid(), SECRET)).await,
        Err(AuthError::Invalid)
    );
    // No kid and no default key: also unknown.
    assert_eq!(auth.authenticate(&token(valid())).await, Err(AuthError::Invalid));
}

struct Outage;
impl KeySource for Outage {
    fn key<'a>(&'a self, _: Option<&'a str>) -> purlin_core::BoxFuture<'a, Result<DecodingKey, KeyError>> {
        Box::pin(async { Err(KeyError::Unavailable("jwks endpoint timed out".into())) })
    }
}

#[tokio::test]
async fn a_key_source_outage_fails_closed_and_garbage_never_reaches_it() {
    let auth = JwtAuthenticator::new(Arc::new(Outage), JwtConfig::hs256());
    assert!(matches!(
        auth.authenticate(&token(valid())).await,
        Err(AuthError::Unavailable(_))
    ));
    // A token that is not even a JWT is rejected without asking the key source.
    assert_eq!(auth.authenticate("garbage").await, Err(AuthError::Invalid));
}

#[tokio::test]
async fn claim_names_are_configurable() {
    let config = JwtConfig::hs256().claims(ClaimMapping {
        roles: "realm_access.roles".into(),
        permissions: vec!["perms".into()],
    });
    let auth = JwtAuthenticator::new(Arc::new(StaticKeys::hmac(SECRET)), config);
    let claims = json!({ "sub": "kc", "exp": now() + 60, "realm_access": { "roles": ["editor"] }, "perms": ["a:b"] });
    let principal = auth.authenticate(&token(claims)).await.unwrap();
    assert!(principal.has_role("editor") && principal.has_permission("a:b"));
}

// A throwaway Ed25519 key pair generated for these tests only.
const ED_PRIVATE: &str = "-----BEGIN PRIVATE KEY-----\nMC4CAQAwBQYDK2VwBCIEIBgdqNv200rpp2+JBQ78PdH4PvH22+F09gZdQxVnSadv\n-----END PRIVATE KEY-----\n";
const ED_PUBLIC: &str = "-----BEGIN PUBLIC KEY-----\nMCowBQYDK2VwAyEAyyISLVDRTpvZ2+fRSf9uuyIqu0fZhrBI+Pb5tgkLOV0=\n-----END PUBLIC KEY-----\n";

#[tokio::test]
async fn asymmetric_keys_work_from_pem_and_a_hmac_secret_cannot_stand_in_for_them() {
    let keys = StaticKeys::pem(ED_PUBLIC, KeyFamily::Ed25519).unwrap();
    let auth = JwtAuthenticator::new(Arc::new(keys), JwtConfig::eddsa());
    let signed = encode(
        &Header::new(Algorithm::EdDSA),
        &valid(),
        &EncodingKey::from_ed_pem(ED_PRIVATE.as_bytes()).unwrap(),
    )
    .unwrap();
    assert_eq!(auth.authenticate(&signed).await.unwrap().subject(), "ann");

    // Algorithm confusion: an HS256 token "signed" with the public key as the secret.
    let confused = token_with(Header::new(Algorithm::HS256), valid(), ED_PUBLIC.as_bytes());
    assert_eq!(auth.authenticate(&confused).await, Err(AuthError::Invalid));
}

#[test]
fn bad_pem_is_reported_at_start_up_not_per_request() {
    assert!(StaticKeys::pem("not a pem", KeyFamily::Rsa).is_err());
    assert!(StaticKeys::jwks("{").is_err());
}

#[test]
fn key_debug_output_hides_material() {
    let text = format!(
        "{:?}",
        StaticKeys::hmac(SECRET).key("k1", DecodingKey::from_secret(b"x"))
    );
    assert!(text.contains("k1") && !text.contains("test-secret"), "{text}");
}

#[test]
#[should_panic(expected = "at least one accepted algorithm")]
fn an_empty_algorithm_list_is_a_programming_error() {
    let _ = JwtConfig::new([]);
}

#[tokio::test(start_paused = true)]
async fn it_composes_with_the_token_cache() {
    use purlin_http::{CacheConfig, cached};
    let port = cached(authenticator(), CacheConfig::default());
    let t = token(valid());
    assert!(port.authenticate(&t).await.is_ok());
    assert!(port.authenticate(&t).await.is_ok());
}
