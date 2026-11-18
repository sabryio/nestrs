//! The error type for describing, validating, starting and stopping an app.

use std::fmt;
use std::time::Duration;

use crate::key::Key;

/// A boxed error: what constructors, invokes and hooks may fail with.
pub type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Shorthand for `Result<T, fx::Error>`.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Which half of the lifecycle something happened in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Building the graph, running invokes and start hooks.
    Start,
    /// Running stop hooks.
    Stop,
}

impl fmt::Display for Phase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Start => "start",
            Self::Stop => "stop",
        })
    }
}

fn trail(path: &[String]) -> String {
    if path.is_empty() {
        String::new()
    } else {
        format!("\n  required via: {}", path.join(" -> "))
    }
}

fn joined(errors: &[Error]) -> String {
    errors.iter().map(|e| format!("\n  - {e}")).collect()
}

/// Everything that can go wrong with an app.
///
/// Wiring mistakes (every variant up to [`Error::UnknownReplace`]) are found by
/// [`App::validate`](crate::App::validate) or at the very start of
/// [`App::start`](crate::App::start), before any constructor has run.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Something needs a value that nobody provides.
    #[error("{requester} needs `{key}`, but nothing provides it{}", trail(.path))]
    Missing {
        /// The value that is missing.
        key: Key,
        /// Who asked for it.
        requester: Box<str>,
        /// The chain of providers from the invoke that pulled this in.
        path: Box<[String]>,
    },

    /// The value exists, but in a module that does not export it to the requester.
    #[error(
        "{requester} needs `{key}`, which module `{provided_in}` provides but does not export \
         to `{scope}` (export it and import that module){}", trail(.path)
    )]
    NotVisible {
        /// The value out of reach.
        key: Key,
        /// Who asked for it.
        requester: Box<str>,
        /// The module that owns the value.
        provided_in: Box<str>,
        /// The module the requester lives in.
        scope: Box<str>,
        /// The chain of providers from the invoke that pulled this in.
        path: Box<[String]>,
    },

    /// Two providers claim the same key in the same scope.
    #[error("`{key}` is provided twice in `{scope}`: by {first} and by {second}")]
    Duplicate {
        /// The contested key.
        key: Key,
        /// The module (or `root`) both live in.
        scope: Box<str>,
        /// The first provider.
        first: Box<str>,
        /// The second provider.
        second: Box<str>,
    },

    /// Several imported modules export the same key.
    #[error("`{key}` is exported by several imported modules ({}); import only one or rename one", .modules.join(", "))]
    Ambiguous {
        /// The contested key.
        key: Key,
        /// The modules that export it.
        modules: Vec<String>,
    },

    /// A module exports something it neither provides nor imports.
    #[error("module `{module}` exports `{key}`, but neither provides it nor imports a module that exports it")]
    InvalidExport {
        /// The offending module.
        module: Box<str>,
        /// The key it tried to export.
        key: Key,
    },

    /// Modules import each other in a loop.
    #[error("circular module import: {}", .path.join(" -> "))]
    ModuleCycle {
        /// Module names along the cycle, first == last.
        path: Vec<String>,
    },

    /// Providers depend on each other in a loop.
    #[error("circular dependency: {}", .path.join(" -> "))]
    Cycle {
        /// Values along the cycle, first == last.
        path: Vec<String>,
    },

    /// A decorator does not take the value it decorates.
    #[error("decorator {decorator} returns `{key}`, so it must also take `{key}` as an argument")]
    DecoratorInput {
        /// The decorator.
        decorator: Box<str>,
        /// The decorated key.
        key: Key,
    },

    /// A replacement targets a key that no provider declares.
    #[error("cannot replace `{key}`: nothing provides it")]
    UnknownReplace {
        /// The key the replacement was meant for.
        key: Key,
    },

    /// A constructor returned an error.
    #[error("{origin} failed to build `{key}`: {source}")]
    Build {
        /// What was being built.
        key: Box<str>,
        /// The constructor.
        origin: Box<str>,
        /// What it failed with.
        #[source]
        source: BoxError,
    },

    /// An invoked function returned an error.
    #[error("invoke {origin} failed: {source}")]
    Invoke {
        /// The invoked function.
        origin: Box<str>,
        /// What it failed with.
        #[source]
        source: BoxError,
    },

    /// A lifecycle hook returned an error.
    #[error("{phase} hook of {owner} failed: {source}")]
    Hook {
        /// The provider that registered the hook.
        owner: Box<str>,
        /// Start or stop.
        phase: Phase,
        /// What it failed with.
        #[source]
        source: BoxError,
    },

    /// Starting or stopping took longer than allowed.
    #[error("app did not {phase} within {limit:?}")]
    Timeout {
        /// Start or stop.
        phase: Phase,
        /// The configured limit.
        limit: Duration,
    },

    /// One or more stop hooks failed; the rest still ran.
    #[error("{} error(s) while stopping:{}", .errors.len(), joined(.errors))]
    Stop {
        /// Every failure, in the order the hooks ran.
        errors: Vec<Error>,
    },

    /// An operation needs a started app.
    #[error("the app is not running")]
    NotStarted,

    /// `start` was called twice.
    #[error("the app is already running")]
    AlreadyStarted,

    /// A broken invariant inside the framework. Please report it.
    #[error("internal error: {0}")]
    Internal(String),
}

impl Error {
    pub(crate) fn internal(message: impl fmt::Display) -> Self {
        Self::Internal(message.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `fx::Result<()>` is what users return from `main` and from their own
    /// functions; clippy's `result_large_err` fires at 128 bytes, so keep headroom.
    #[test]
    fn the_error_type_stays_small_enough_to_return_everywhere() {
        assert!(
            std::mem::size_of::<Error>() < 128,
            "{} bytes",
            std::mem::size_of::<Error>()
        );
    }
}
