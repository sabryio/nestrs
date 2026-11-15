use std::any::Any;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use crate::{DiError, Token};

/// A built provider, type-erased.
///
/// The stored object is always an `Arc<T>` (itself wrapped in an `Arc`), where `T`
/// may be a trait object. Wrapping the `Arc` is what lets `dyn Trait` providers be
/// stored in a `dyn Any` map: `Arc<dyn Trait>` is sized and `'static`, `dyn Trait`
/// alone is not.
pub(crate) type Stored = Arc<dyn Any + Send + Sync>;

/// Holds every singleton provider after bootstrap.
///
/// The container is deliberately dumb: it knows nothing about modules, visibility
/// or ordering. Those concerns live in the graph validation that runs before the
/// container is populated.
#[derive(Default)]
pub struct Container {
    entries: HashMap<Token, Stored>,
}

impl Container {
    /// Creates an empty container.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns a shared handle to the provider registered for `T`.
    ///
    /// `T` may be a concrete type or a trait object (`dyn Repo`).
    pub fn resolve<T>(&self) -> Result<Arc<T>, DiError>
    where
        T: ?Sized + Send + Sync + 'static,
    {
        let token = Token::of::<T>();
        self.entries
            .get(&token)
            .and_then(|stored| stored.downcast_ref::<Arc<T>>())
            .cloned()
            .ok_or(DiError::NotFound { requested: token })
    }

    /// Whether a provider for `T` has been registered.
    pub fn contains<T: ?Sized + 'static>(&self) -> bool {
        self.entries.contains_key(&Token::of::<T>())
    }

    /// Registers a ready-made value, replacing any previous provider for `T`.
    ///
    /// Mostly useful in tests and for hand-assembled containers; applications
    /// normally let bootstrap fill the container.
    pub fn insert<T>(&mut self, value: Arc<T>)
    where
        T: ?Sized + Send + Sync + 'static,
    {
        self.insert_raw(Token::of::<T>(), Arc::new(value));
    }

    /// Number of registered providers.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the container has no providers.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub(crate) fn get_raw(&self, token: Token) -> Option<Stored> {
        self.entries.get(&token).cloned()
    }

    pub(crate) fn insert_raw(&mut self, token: Token, value: Stored) {
        self.entries.insert(token, value);
    }
}

impl fmt::Debug for Container {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut names: Vec<String> = self.entries.keys().map(Token::to_string).collect();
        names.sort();
        f.debug_struct("Container").field("providers", &names).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    trait Speaker: Send + Sync {
        fn speak(&self) -> &'static str;
    }
    struct Dog;
    impl Speaker for Dog {
        fn speak(&self) -> &'static str {
            "woof"
        }
    }

    #[test]
    fn stores_and_resolves_concrete_and_dyn() {
        let mut container = Container::new();
        container.insert::<Dog>(Arc::new(Dog));
        container.insert::<dyn Speaker>(Arc::new(Dog));

        assert!(container.contains::<Dog>());
        assert_eq!(container.resolve::<dyn Speaker>().unwrap().speak(), "woof");
        assert_eq!(container.len(), 2);
    }

    #[test]
    fn missing_provider_is_a_typed_error() {
        let container = Container::new();
        assert!(matches!(container.resolve::<Dog>(), Err(DiError::NotFound { .. })));
    }
}
