//! Static-token implementation of the framework's `Authenticator` port.

use std::sync::Arc;

use nestrs::BoxFuture;
use nestrs::http::{AuthError, Authenticator, AuthenticatorPort, Principal, StaticTokens};
use nestrs::prelude::*;

use super::config::AppConfig;

/// Adapter for [`Authenticator`]: the admin and author tokens configured in
/// [`AppConfig`]. Swapping in JWT verification means a new adapter and one
/// `provides = ..`; guards, controllers and tests do not change.
#[injectable(provides = AuthenticatorPort)]
pub struct StaticTokenVerifier {
    config: Arc<AppConfig>,
}

impl StaticTokenVerifier {
    /// The configured tokens and who they stand for.
    fn tokens(&self) -> StaticTokens {
        StaticTokens::new()
            .token(
                self.config.api_token.clone(),
                Principal::new("admin").with_roles(["admin"]).with_permissions(["*"]),
            )
            .token(
                self.config.author_token.clone(),
                Principal::new("author")
                    .with_roles(["author"])
                    .with_permissions(["posts:create"]),
            )
    }
}

impl Authenticator for StaticTokenVerifier {
    fn authenticate<'a>(&'a self, token: &'a str) -> BoxFuture<'a, Result<Principal, AuthError>> {
        // `StaticTokens` compares in constant time.
        let tokens = self.tokens();
        Box::pin(async move { tokens.authenticate(token).await })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verifier(token: &str) -> StaticTokenVerifier {
        StaticTokenVerifier {
            config: Arc::new(AppConfig {
                api_token: token.to_owned(),
                ..AppConfig::default()
            }),
        }
    }

    #[tokio::test]
    async fn accepts_only_the_configured_tokens() {
        let verifier = verifier("s3cret");
        assert_eq!(verifier.authenticate("s3cret").await.unwrap().subject(), "admin");
        assert_eq!(verifier.authenticate("wrong").await, Err(AuthError::Invalid));
        assert!(verifier.authenticate("author-token").await.unwrap().has_role("author"));
        assert_eq!(verifier.authenticate("").await, Err(AuthError::Invalid));
    }
}
