//! Authorization: who is calling ([`Principal`]) and what a route demands of them
//! ([`Requirement`]).
//!
//! Authentication (turning a token into a `Principal`) is the application's job and
//! lives in an ordinary [`Guard`](crate::Guard) that stores the result in the request
//! extensions. Authorization is declarative and generic:
//!
//! ```ignore
//! #[controller("/posts")]
//! #[guard(AuthGuard)]                       // authenticates, inserts a Principal
//! impl PostsController {
//!     #[post]
//!     #[roles("admin", "author")]           // any of these roles
//!     async fn create(&self, ..) { .. }
//!
//!     #[delete("/{id}")]
//!     #[permissions("posts:delete")]        // all of these permissions
//!     async fn remove(&self, principal: Principal, ..) { .. }
//! }
//! ```
//!
//! ## Semantics
//!
//! * No `Principal` on the request: **401**. A principal that lacks what the route
//!   needs: **403**. Neither response says what was required.
//! * `#[roles(a, b)]` is satisfied by *any* of the roles. `#[permissions(x, y)]` needs
//!   *all* of them. Several attributes, on the controller and on the route, all have to hold.
//! * Permissions are `namespace:action` strings. A granted `posts:*` covers every
//!   `posts:` permission and a granted `*` covers all of them.
//! * Requirements run after the controller's and route's `#[guard]`s (which
//!   authenticate) and before interceptors.
//!
//! ## Design
//!
//! Single responsibility: the guard authenticates, [`Requirement`] decides, the
//! middleware only maps the decision to HTTP. Open/closed: a new identity source is a
//! new guard, and roles/permissions need no change.

use std::collections::BTreeSet;
use std::fmt;

use axum::extract::{FromRequestParts, Request};
use axum::http::request::Parts;
use axum::middleware::{self, Next};
use axum::response::IntoResponse;
use axum::routing::MethodRouter;

use crate::HttpError;

/// The authenticated caller: an id plus what they are allowed to do.
///
/// Insert one into the request extensions from your authentication guard
/// (`parts.extensions.insert(principal)`); handlers then take it as a parameter.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Principal {
    subject: String,
    roles: BTreeSet<String>,
    permissions: BTreeSet<String>,
}

impl Principal {
    /// A caller identified by `subject` (user id, service name) with no privileges.
    pub fn new(subject: impl Into<String>) -> Self {
        Self {
            subject: subject.into(),
            ..Self::default()
        }
    }

    /// Adds roles.
    pub fn with_roles<S: Into<String>>(mut self, roles: impl IntoIterator<Item = S>) -> Self {
        self.roles.extend(roles.into_iter().map(Into::into));
        self
    }

    /// Adds permissions (`"posts:delete"`, `"posts:*"`, `"*"`).
    pub fn with_permissions<S: Into<String>>(mut self, permissions: impl IntoIterator<Item = S>) -> Self {
        self.permissions.extend(permissions.into_iter().map(Into::into));
        self
    }

    /// Who this is.
    pub fn subject(&self) -> &str {
        &self.subject
    }

    /// The roles held.
    pub fn roles(&self) -> impl Iterator<Item = &str> {
        self.roles.iter().map(String::as_str)
    }

    /// Whether the caller has exactly this role.
    pub fn has_role(&self, role: &str) -> bool {
        self.roles.contains(role)
    }

    /// Whether the caller has at least one of `roles`.
    pub fn has_any_role(&self, roles: &[&str]) -> bool {
        roles.iter().any(|role| self.has_role(role))
    }

    /// Whether a granted permission covers `required`; see the module docs for wildcards.
    pub fn has_permission(&self, required: &str) -> bool {
        self.permissions.iter().any(|granted| grants(granted, required))
    }

    /// Whether the caller holds every one of `permissions`.
    pub fn has_all_permissions(&self, permissions: &[&str]) -> bool {
        permissions.iter().all(|required| self.has_permission(required))
    }
}

/// `*` covers everything; `ns:*` covers `ns:` followed by anything; otherwise exact.
fn grants(granted: &str, required: &str) -> bool {
    if granted == "*" || granted == required {
        return true;
    }
    granted
        .strip_suffix('*')
        .is_some_and(|prefix| prefix.ends_with(':') && required.starts_with(prefix))
}

impl<S: Send + Sync> FromRequestParts<S> for Principal {
    type Rejection = HttpError;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<Self>()
            .cloned()
            .ok_or_else(|| HttpError::unauthorized("authentication required"))
    }
}

/// `Option<Principal>` for routes behind an optional guard: `None` when nobody signed in.
impl<S: Send + Sync> axum::extract::OptionalFromRequestParts<S> for Principal {
    type Rejection = HttpError;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Option<Self>, Self::Rejection> {
        Ok(parts.extensions.get::<Self>().cloned())
    }
}

/// What a route demands of its caller. Built by `#[roles]` / `#[permissions]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Requirement {
    /// At least one of these roles.
    AnyRole(&'static [&'static str]),
    /// Every one of these permissions.
    AllPermissions(&'static [&'static str]),
}

impl Requirement {
    /// Whether `principal` satisfies this requirement.
    pub fn is_met_by(&self, principal: &Principal) -> bool {
        match self {
            Self::AnyRole(roles) => principal.has_any_role(roles),
            Self::AllPermissions(permissions) => principal.has_all_permissions(permissions),
        }
    }
}

impl fmt::Display for Requirement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AnyRole(roles) => write!(f, "role {}", roles.join(" or ")),
            Self::AllPermissions(permissions) => write!(f, "permission {}", permissions.join(" and ")),
        }
    }
}

/// Wraps `route` so `requirement` is enforced. Used by `#[controller]`-generated code.
pub fn authorized(route: MethodRouter, requirement: Requirement) -> MethodRouter {
    route.route_layer(middleware::from_fn(move |request: Request, next: Next| async move {
        let Some(principal) = request.extensions().get::<Principal>() else {
            return HttpError::unauthorized("authentication required").into_response();
        };
        if !requirement.is_met_by(principal) {
            // The client learns nothing about what would have been enough.
            tracing::warn!(subject = principal.subject(), %requirement, "forbidden");
            return HttpError::forbidden("you are not allowed to do this").into_response();
        }
        next.run(request).await
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_and_wildcard_permissions() {
        let p = Principal::new("u").with_permissions(["posts:delete", "users:*"]);
        assert!(p.has_permission("posts:delete"));
        assert!(!p.has_permission("posts:create"));
        assert!(p.has_permission("users:create") && p.has_permission("users:delete"));
        assert!(!p.has_permission("users"), "a namespace wildcard needs the colon");
        assert!(
            Principal::new("root")
                .with_permissions(["*"])
                .has_permission("anything:at-all")
        );
        assert!(
            !Principal::new("u")
                .with_permissions(["post*"])
                .has_permission("posts:x"),
            "no loose prefixes"
        );
    }

    #[test]
    fn roles_are_any_of_and_permissions_are_all_of() {
        let p = Principal::new("u")
            .with_roles(["author"])
            .with_permissions(["a:1", "a:2"]);
        assert!(Requirement::AnyRole(&["admin", "author"]).is_met_by(&p));
        assert!(!Requirement::AnyRole(&["admin"]).is_met_by(&p));
        assert!(Requirement::AllPermissions(&["a:1", "a:2"]).is_met_by(&p));
        assert!(!Requirement::AllPermissions(&["a:1", "a:3"]).is_met_by(&p));
    }

    #[test]
    fn requirements_read_naturally() {
        assert_eq!(
            Requirement::AnyRole(&["admin", "author"]).to_string(),
            "role admin or author"
        );
        assert_eq!(
            Requirement::AllPermissions(&["a:1", "b:2"]).to_string(),
            "permission a:1 and b:2"
        );
    }
}
