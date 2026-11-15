//! Turning verified claims into a [`Principal`].

use nestrs_http::Principal;
use serde_json::Value;

/// Which claims carry the caller's roles and permissions.
///
/// Names may be dotted paths into nested objects (`"realm_access.roles"`, the
/// Keycloak layout). A claim may be an array of strings or one space-separated string
/// (the OAuth2 `scope` format).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimMapping {
    /// Claim holding roles. Default `roles`.
    pub roles: String,
    /// Claims holding permissions; all of them are merged. Default `permissions` and `scope`.
    pub permissions: Vec<String>,
}

impl Default for ClaimMapping {
    fn default() -> Self {
        Self {
            roles: "roles".to_owned(),
            permissions: vec!["permissions".to_owned(), "scope".to_owned()],
        }
    }
}

impl ClaimMapping {
    /// Builds the principal for `subject` from `claims`.
    pub(crate) fn principal(&self, subject: &str, claims: &Value) -> Principal {
        Principal::new(subject)
            .with_roles(strings(claims, &self.roles))
            .with_permissions(self.permissions.iter().flat_map(|claim| strings(claims, claim)))
    }
}

/// The strings under a dotted `path`; absent or oddly typed claims yield nothing.
fn strings(claims: &Value, path: &str) -> Vec<String> {
    let value = path.split('.').try_fold(claims, |value, key| value.get(key));
    match value {
        Some(Value::String(text)) => text.split_whitespace().map(str::to_owned).collect(),
        Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).map(str::to_owned).collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn arrays_space_separated_strings_and_nested_paths_all_work() {
        let claims = json!({
            "roles": ["admin", "author"],
            "scope": "posts:read posts:create",
            "permissions": ["audit:write", 7, null],
            "realm_access": { "roles": ["kc-admin"] },
        });
        let principal = ClaimMapping::default().principal("u1", &claims);
        assert!(principal.has_role("admin") && principal.has_role("author"));
        assert!(principal.has_permission("posts:create") && principal.has_permission("audit:write"));
        assert!(!principal.has_role("kc-admin"));

        let keycloak = ClaimMapping {
            roles: "realm_access.roles".into(),
            ..ClaimMapping::default()
        };
        assert!(keycloak.principal("u1", &claims).has_role("kc-admin"));
    }

    #[test]
    fn missing_or_misshapen_claims_grant_nothing() {
        let claims = json!({ "roles": { "not": "a list" }, "scope": 5 });
        let principal = ClaimMapping::default().principal("u1", &claims);
        assert_eq!(principal.roles().count(), 0);
        assert!(!principal.has_permission("anything"));
    }
}
