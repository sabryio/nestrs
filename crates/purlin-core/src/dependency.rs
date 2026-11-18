//! The vocabulary that lets declarations say *what* they need without spelling out
//! how it is stored.
//!
//! - [`Dependency`]: a handle the container can hand out, i.e. `Arc<T>`. Because
//!   [`#[port]`](https://docs.rs/purlin) generates the alias
//!   `type UserRepositoryPort = Arc<dyn UserRepository>`, a field of that alias type
//!   is a dependency on the trait object, and nobody writes `dyn`.
//! - [`ProviderSource`]: anything a module can list under `providers`: an
//!   `#[injectable]` type, or a `#[provider]` function.
//!
//! The `__private` module holds helpers for generated code. It is not part of the
//! public API and may change without notice.

use std::sync::Arc;

use crate::{Container, DiError, Injectable, Provider, Token};

/// A handle to a provider that the container can resolve: `Arc<T>`, where `T` is a
/// concrete type or a `dyn Trait`.
///
/// Implemented for `Arc<T>` only. Type aliases such as `UserRepositoryPort` work
/// because they *are* `Arc<dyn UserRepository>`; resolution is driven by the type,
/// not by how it is spelled.
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be injected",
    label = "not an injectable handle",
    note = "inject `Arc<T>` (or a port alias such as `UserRepositoryPort`); mark non-injected fields with `#[inject(default)]`"
)]
pub trait Dependency: Sized + Send + Sync + 'static {
    /// The provided type: `T` for `Arc<T>`.
    type Target: ?Sized + Send + Sync + 'static;

    /// The container key of [`Target`](Self::Target).
    fn token() -> Token {
        Token::of::<Self::Target>()
    }

    /// Resolves the handle from the container.
    fn resolve(container: &Container) -> Result<Self, DiError>;

    /// Turns the handle into the shared pointer the container stores.
    fn into_shared(self) -> Arc<Self::Target>;
}

impl<T> Dependency for Arc<T>
where
    T: ?Sized + Send + Sync + 'static,
{
    type Target = T;

    fn resolve(container: &Container) -> Result<Self, DiError> {
        container.resolve::<T>()
    }

    fn into_shared(self) -> Arc<T> {
        self
    }
}

/// Something a module can list under `providers`.
///
/// Every [`Injectable`] is one (its [`Injectable::register`] decides what it
/// registers); `#[provider]` functions implement it through a generated marker type.
pub trait ProviderSource {
    /// The providers to register.
    fn providers() -> Vec<Provider>;
}

impl<T: Injectable> ProviderSource for T {
    fn providers() -> Vec<Provider> {
        T::register()
    }
}

/// Helpers for macro-generated code.
#[doc(hidden)]
pub mod __private {
    use std::marker::PhantomData;
    use std::sync::Arc;

    pub use super::{Dependency, ProviderSource};
    use crate::provider::BoxFuture;
    use crate::{Container, DiError, OnModuleDestroy, OnModuleInit, Provider, Token};

    // Autoref specialisation: `(&Probe::<T>::new()).method()` calls the `*Via*` impl
    // when the bound holds and the fallback otherwise. It lets one macro expansion
    // adapt to what the user's type implements, without the user saying so.

    /// Type-level probe.
    pub struct Probe<T: ?Sized>(PhantomData<fn() -> Box<T>>);

    impl<T: ?Sized> Probe<T> {
        /// Creates the probe.
        pub fn new() -> Self {
            Self(PhantomData)
        }
    }

    impl<T: ?Sized> Default for Probe<T> {
        fn default() -> Self {
            Self::new()
        }
    }

    // ---- tokens: `Arc<dyn X>` (a port alias) and `X` / `dyn X` map to one key

    /// `T` is an injectable handle: its token is its target's.
    pub trait TokenViaDependency {
        /// The container key.
        fn token(&self) -> Token;
    }
    impl<T: Dependency> TokenViaDependency for Probe<T> {
        fn token(&self) -> Token {
            T::token()
        }
    }

    /// Fallback: the type itself is the key.
    pub trait TokenViaType {
        /// The container key.
        fn token(&self) -> Token;
    }
    impl<T: ?Sized + 'static> TokenViaType for &Probe<T> {
        fn token(&self) -> Token {
            Token::of::<T>()
        }
    }

    // ---- lifecycle: attach hooks when the type implements them

    /// `T: OnModuleInit`.
    pub trait InitViaTrait {
        /// Attaches the init hook.
        fn attach_init(&self, provider: Provider) -> Provider;
        /// Whether a hook exists.
        fn has_init(&self) -> bool;
    }
    impl<T: OnModuleInit> InitViaTrait for Probe<T> {
        fn attach_init(&self, provider: Provider) -> Provider {
            provider.with_init::<T>()
        }
        fn has_init(&self) -> bool {
            true
        }
    }
    /// Fallback: no init hook.
    pub trait InitSkipped {
        /// Leaves the provider as it is.
        fn attach_init(&self, provider: Provider) -> Provider {
            provider
        }
        /// Whether a hook exists.
        fn has_init(&self) -> bool {
            false
        }
    }
    impl<T> InitSkipped for &Probe<T> {}

    /// `T: OnModuleDestroy`.
    pub trait DestroyViaTrait {
        /// Attaches the destroy hook.
        fn attach_destroy(&self, provider: Provider) -> Provider;
        /// Whether a hook exists.
        fn has_destroy(&self) -> bool;
    }
    impl<T: OnModuleDestroy> DestroyViaTrait for Probe<T> {
        fn attach_destroy(&self, provider: Provider) -> Provider {
            provider.with_destroy::<T>()
        }
        fn has_destroy(&self) -> bool {
            true
        }
    }
    /// Fallback: no destroy hook.
    pub trait DestroySkipped {
        /// Leaves the provider as it is.
        fn attach_destroy(&self, provider: Provider) -> Provider {
            provider
        }
        /// Whether a hook exists.
        fn has_destroy(&self) -> bool {
            false
        }
    }
    impl<T> DestroySkipped for &Probe<T> {}

    // ---- `#[provider]` functions: `Arc<T>` / port aliases vs plain values

    /// The function returns a shared handle (`Arc<T>` or a port alias).
    pub trait OutputViaHandle<T: ?Sized> {
        /// Builds the provider.
        fn provide_with<F>(&self, dependencies: Vec<Token>, factory: F) -> Provider
        where
            F: for<'a> Fn(&'a Container) -> BoxFuture<'a, Result<Arc<T>, DiError>> + Send + Sync + 'static;
    }
    impl<T: ?Sized + Send + Sync + 'static> OutputViaHandle<T> for Probe<Arc<T>> {
        fn provide_with<F>(&self, dependencies: Vec<Token>, factory: F) -> Provider
        where
            F: for<'a> Fn(&'a Container) -> BoxFuture<'a, Result<Arc<T>, DiError>> + Send + Sync + 'static,
        {
            Provider::factory_arc::<T>(dependencies, factory)
        }
    }

    /// Fallback: the function returns a plain value.
    pub trait OutputViaValue<R> {
        /// Builds the provider.
        fn provide_with<F>(&self, dependencies: Vec<Token>, factory: F) -> Provider
        where
            F: for<'a> Fn(&'a Container) -> BoxFuture<'a, Result<R, DiError>> + Send + Sync + 'static;
    }
    impl<R: Send + Sync + 'static> OutputViaValue<R> for &Probe<R> {
        fn provide_with<F>(&self, dependencies: Vec<Token>, factory: F) -> Provider
        where
            F: for<'a> Fn(&'a Container) -> BoxFuture<'a, Result<R, DiError>> + Send + Sync + 'static,
        {
            Provider::factory::<R>(dependencies, factory)
        }
    }

    /// Registers the adapter `T` under the port `P`.
    ///
    /// `concrete` is `T`'s own provider with its lifecycle hooks attached, and
    /// `hooked` says whether it has any. Probing happens in the generated code,
    /// where `T` is a concrete type; inside a generic function the probe could not
    /// see `T`'s trait implementations.
    ///
    /// Without hooks this is a single provider (the port), so overriding the port in
    /// a test never builds the real adapter. With hooks, the adapter is registered
    /// too, so the hooks have an instance to run on, and the port resolves to it.
    pub fn register_bound<P, T>(coerce: fn(Arc<T>) -> Arc<P::Target>, hooked: bool, concrete: Provider) -> Vec<Provider>
    where
        P: Dependency,
        T: crate::Injectable,
    {
        if !hooked {
            return vec![Provider::bind::<P::Target, T>(coerce)];
        }
        let port = Provider::factory_arc::<P::Target>(vec![Token::of::<T>()], move |container| {
            Box::pin(async move { Ok(coerce(container.resolve::<T>()?)) })
        });
        vec![concrete, port]
    }
}
