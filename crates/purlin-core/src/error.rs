use crate::Token;

/// Everything that can go wrong while describing, validating or building the
/// dependency graph.
///
/// All variants except [`DiError::NotFound`] are produced at bootstrap, before the
/// application serves a single request.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DiError {
    /// A lookup against the built container found nothing.
    #[error("no provider registered for `{requested}`")]
    NotFound {
        /// The type that was requested.
        requested: Token,
    },

    /// A provider depends on a type that no module provides.
    #[error("`{requester}` (module `{module}`) depends on `{dependency}`, but no module provides it")]
    MissingProvider {
        /// The provider with the unmet dependency.
        requester: Token,
        /// The module that declares `requester`.
        module: &'static str,
        /// The type nobody provides.
        dependency: Token,
    },

    /// A dependency exists, but is not reachable from the requesting module.
    #[error(
        "`{requester}` (module `{module}`) depends on `{dependency}`, which module \
         `{provided_in}` provides but does not export to `{module}` \
         (export it and import that module, or make `{provided_in}` global)"
    )]
    NotVisible {
        /// The provider with the unreachable dependency.
        requester: Token,
        /// The module that declares `requester`.
        module: &'static str,
        /// The type that is out of reach.
        dependency: Token,
        /// The module that owns `dependency`.
        provided_in: &'static str,
    },

    /// Two modules provide the same type.
    #[error("`{token}` is provided by both module `{first}` and module `{second}`")]
    DuplicateProvider {
        /// The contested type.
        token: Token,
        /// Module that registered it first.
        first: &'static str,
        /// Module that tried to register it again.
        second: &'static str,
    },

    /// A module exports something it neither provides nor re-exports.
    #[error("module `{module}` exports `{token}`, but neither provides it nor imports a module that exports it")]
    InvalidExport {
        /// The offending module.
        module: &'static str,
        /// The type it tried to export.
        token: Token,
    },

    /// Modules import each other in a loop.
    #[error("circular module import: {}", .path.join(" -> "))]
    ModuleCycle {
        /// Module names along the cycle, first == last.
        path: Vec<String>,
    },

    /// Providers depend on each other in a loop.
    #[error("circular dependency: {}", .path.join(" -> "))]
    CircularDependency {
        /// Provider names along the cycle, first == last.
        path: Vec<String>,
    },

    /// An override targets a type that no module provides.
    #[error("cannot override `{token}`: no module provides it")]
    UnknownOverride {
        /// The type the override was meant to replace.
        token: Token,
    },

    /// A singleton depends on a request-scoped or transient provider.
    #[error(
        "{provider_lifetime} `{provider}` cannot depend on {dependency_lifetime} `{dependency}`: \
         make `{provider}` request-scoped too, or `{dependency}` a singleton"
    )]
    ScopeMismatch {
        /// The longer-lived provider.
        provider: Token,
        /// Its lifetime.
        provider_lifetime: crate::Lifetime,
        /// The shorter-lived dependency.
        dependency: Token,
        /// Its lifetime.
        dependency_lifetime: crate::Lifetime,
    },

    /// A provider's factory returned an error while building.
    #[error("provider `{provider}` failed to build: {message}")]
    Factory {
        /// The provider that failed.
        provider: Token,
        /// What went wrong.
        message: String,
    },

    /// A lifecycle hook (`init` or `destroy`) of a provider failed.
    #[error("provider `{provider}` failed during {phase}: {message}")]
    Hook {
        /// The provider whose hook failed.
        provider: Token,
        /// `"init"` or `"destroy"`.
        phase: &'static str,
        /// What went wrong.
        message: String,
    },

    /// Free-form error for user-written factories; bootstrap wraps it in
    /// [`DiError::Factory`] with the provider's name attached.
    #[error("{0}")]
    Custom(String),
}

impl DiError {
    /// Builds a [`DiError::Custom`] from anything displayable.
    pub fn custom(message: impl std::fmt::Display) -> Self {
        Self::Custom(message.to_string())
    }
}
