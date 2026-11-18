//! [`Options`]: an ordered, mergeable list of everything an app is made of.

use std::panic::Location;
use std::sync::Arc;

use crate::descriptor::{Decorator, IntoDecorator, IntoInvoke, IntoProvider, Invoke, Provider};
use crate::func::Component;
use crate::key::Key;
use crate::module::{Module, ModuleRef};
use crate::value::{Stored, store};

#[derive(Clone)]
pub(crate) enum Item {
    Provide(Provider),
    Invoke(Invoke),
    Decorate(Decorator),
    Module(ModuleRef),
    Replace(Replacement),
}

#[derive(Clone)]
pub(crate) struct Replacement {
    pub(crate) key: Key,
    pub(crate) value: Stored,
    pub(crate) location: &'static Location<'static>,
}

/// An ordered list of providers, invokes, decorators and modules.
///
/// It is both what [`App`](crate::App) is built from and the unit of reuse: build an
/// `Options` in one place, hand it around, and [`merge`](Self::merge) it into an app
/// or another module (fx's `fx.Options`).
#[derive(Clone, Default)]
pub struct Options {
    pub(crate) items: Vec<Item>,
}

impl Options {
    /// An empty list.
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn push(mut self, item: Item) -> Self {
        self.items.push(item);
        self
    }

    /// Registers a constructor (`fx.Provide`).
    ///
    /// When it provides the plain value of a type that has `#[fx::bind]` declarations,
    /// the interface providers are registered right after it, in the same scope.
    #[must_use]
    #[track_caller]
    pub fn constructor<M>(self, provider: impl IntoProvider<M>) -> Self {
        let provider = provider.into_provider();
        let plain = provider.key.plain_type_id();
        let options = self.push(Item::Provide(provider));
        match plain {
            Some(id) => crate::bind::apply_declared(options, id),
            None => options,
        }
    }

    /// Registers a function to run at startup (`fx.Invoke`).
    #[must_use]
    #[track_caller]
    pub fn invoke<M>(self, invoke: impl IntoInvoke<M>) -> Self {
        self.push(Item::Invoke(invoke.into_invoke()))
    }

    /// Registers a decorator (`fx.Decorate`).
    #[must_use]
    #[track_caller]
    pub fn decorate<M>(self, decorator: impl IntoDecorator<M>) -> Self {
        self.push(Item::Decorate(decorator.into_decorator()))
    }

    /// Provides an existing value as-is (`fx.Supply`). Consumers take `Arc<T>`.
    #[must_use]
    #[track_caller]
    pub fn supply<T: Send + Sync + 'static>(self, value: T) -> Self {
        self.supply_arc(Arc::new(value))
    }

    /// Provides an existing shared value, which may be a trait object:
    /// `supply_arc::<dyn Clock>(Arc::new(SystemClock))`.
    #[must_use]
    #[track_caller]
    pub fn supply_arc<T: ?Sized + Send + Sync + 'static>(self, value: Arc<T>) -> Self {
        let provider = Provider::value(value, Key::of::<T>(), "supply", Location::caller());
        self.push(Item::Provide(provider))
    }

    /// Imports a module.
    #[must_use]
    pub fn module<M: Module>(self) -> Self {
        self.push(Item::Module(ModuleRef::of::<M>()))
    }

    /// Registers everything a [`Component`] declares (`#[fx::component]`).
    #[must_use]
    pub fn component<C: Component>(self) -> Self {
        C::register(self)
    }

    /// Additionally provides `C` as the interface `I` (see [`bind`](crate::bind())), for
    /// code that does not use `#[fx::bind]`'s automatic registration. Needs `C: Bound<I>`.
    #[must_use]
    #[track_caller]
    pub fn bind<I: ?Sized + Send + Sync + 'static, C: crate::Bound<I>>(self) -> Self {
        self.constructor(crate::bind::<I, C>())
    }

    /// Appends another list.
    #[must_use]
    pub fn merge(mut self, other: Options) -> Self {
        self.items.extend(other.items);
        self
    }

    /// Replaces whatever provides `T` with `value`, wherever it is declared and with
    /// no dependencies of its own. Meant for tests: swap the database for a fake
    /// without touching the modules that provide it.
    ///
    /// Starting fails with [`Error::UnknownReplace`](crate::Error::UnknownReplace) if
    /// nothing provides `T`.
    #[must_use]
    #[track_caller]
    pub fn replace<T: ?Sized + Send + Sync + 'static>(self, value: Arc<T>) -> Self {
        self.push(Item::Replace(Replacement {
            key: Key::of::<T>(),
            value: store(value),
            location: Location::caller(),
        }))
    }

    /// Like [`replace`](Self::replace), for a value registered under `name`.
    #[must_use]
    #[track_caller]
    pub fn replace_named<T: ?Sized + Send + Sync + 'static>(self, name: &'static str, value: Arc<T>) -> Self {
        self.push(Item::Replace(Replacement {
            key: Key::named::<T>(name),
            value: store(value),
            location: Location::caller(),
        }))
    }
}

impl std::fmt::Debug for Options {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Options({} items)", self.items.len())
    }
}
