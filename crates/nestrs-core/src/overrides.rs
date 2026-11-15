use crate::Provider;

/// Replacement providers applied on top of a module graph, mainly for tests.
///
/// Each override replaces whatever provider the graph already has for the same
/// [`Token`](crate::Token), in place: it keeps the original's module, so exports and
/// importers are unaffected. The rules are deliberately strict:
///
/// - Overriding something no module provides is an error
///   ([`DiError::UnknownOverride`](crate::DiError::UnknownOverride)), so a stale
///   override cannot silently stop doing anything.
/// - An override's own dependencies must exist, but they are *not* held to module
///   visibility; a test double may reach for whatever it needs.
/// - If a token is overridden twice, the last override wins.
///
/// ```
/// # use nestrs_core::{Overrides, Provider};
/// struct Config(&'static str);
/// let overrides = Overrides::new().provide(Provider::value(Config("test")));
/// # let _ = overrides;
/// ```
#[derive(Debug, Default, Clone)]
pub struct Overrides {
    providers: Vec<Provider>,
}

impl Overrides {
    /// No overrides.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a replacement provider.
    pub fn provide(mut self, provider: Provider) -> Self {
        self.providers.push(provider);
        self
    }

    /// Appends `other`'s overrides after this set's, so `other` wins on conflicts.
    pub fn extend(mut self, other: Overrides) -> Self {
        self.providers.extend(other.providers);
        self
    }

    pub(crate) fn into_providers(self) -> Vec<Provider> {
        self.providers
    }
}
