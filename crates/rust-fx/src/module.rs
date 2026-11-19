//! Modules: named, reusable bundles of options with a private scope.

use std::any::TypeId;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::key::Key;
use crate::options::Options;

/// A reusable bundle of providers, invokes and decorators with its own scope.
///
/// What a module provides is visible inside the module only, unless it is listed in
/// [`ModuleDef::export`]; importers see the exports. Things provided at the app root
/// are visible everywhere. Implemented by `#[fx::module]`.
pub trait Module: 'static {
    /// Describes the module.
    fn definition() -> ModuleDef;

    /// This module as [`Options`], ready for [`App::options`](crate::App::options).
    fn options() -> Options
    where
        Self: Sized,
    {
        Options::new().module::<Self>()
    }
}

/// What makes two imports "the same module".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ModuleId {
    /// A [`Module`] type: every `ModuleRef::of::<M>()` is the same module.
    Type(TypeId),
    /// A configured instance: clones of one [`ModuleRef`] are the same module,
    /// two separately built ones are not.
    Instance(u64),
}

#[derive(Clone)]
enum Source {
    Static(fn() -> ModuleDef),
    Dynamic(Arc<ModuleDef>),
}

/// A lazy reference to a module, so that import cycles can be reported instead of
/// recursing forever.
///
/// Importing the same module from several places builds it once. For a [`Module`]
/// type identity is the type; for a configured module ([`ModuleDef::into_ref`])
/// identity is the instance, so clone one `ModuleRef` to share it.
#[derive(Clone)]
pub struct ModuleRef {
    pub(crate) id: ModuleId,
    pub(crate) name: &'static str,
    source: Source,
}

impl ModuleRef {
    /// A reference to module `M`.
    pub fn of<M: Module>() -> Self {
        Self {
            id: ModuleId::Type(TypeId::of::<M>()),
            name: std::any::type_name::<M>(),
            source: Source::Static(M::definition),
        }
    }

    pub(crate) fn define(&self) -> ModuleDef {
        match &self.source {
            Source::Static(definition) => definition(),
            Source::Dynamic(definition) => ModuleDef::clone(definition),
        }
    }
}

impl std::fmt::Debug for ModuleRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ModuleRef({})", self.name)
    }
}

/// What a module is made of.
#[derive(Clone)]
pub struct ModuleDef {
    pub(crate) name: &'static str,
    pub(crate) global: bool,
    pub(crate) imports: Vec<ModuleRef>,
    pub(crate) options: Options,
    pub(crate) exports: Vec<Key>,
}

impl ModuleDef {
    /// An empty module called `name`.
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            global: false,
            imports: Vec::new(),
            options: Options::new(),
            exports: Vec::new(),
        }
    }

    /// Makes this module's exports visible to every module in the graph without an
    /// explicit import (configuration, logging, a database pool).
    ///
    /// The module must still be part of the graph: import it once, typically from
    /// the root.
    #[must_use]
    pub fn global(mut self) -> Self {
        self.global = true;
        self
    }

    /// Turns a runtime-built definition into an importable reference: the building
    /// block of configurable modules (`DatabaseModule::for_root(config)`).
    ///
    /// Clone the returned reference to import one instance from several places.
    /// Two separate calls make two instances; if both provide the same key the
    /// planner reports [`Error::Duplicate`](crate::Error) rather than silently
    /// picking a configuration.
    #[must_use]
    pub fn into_ref(self) -> ModuleRef {
        static NEXT_INSTANCE: AtomicU64 = AtomicU64::new(0);
        ModuleRef {
            id: ModuleId::Instance(NEXT_INSTANCE.fetch_add(1, Ordering::Relaxed)),
            name: self.name,
            source: Source::Dynamic(Arc::new(self)),
        }
    }

    /// Imports another module: its exports become visible here.
    #[must_use]
    pub fn import(mut self, module: ModuleRef) -> Self {
        self.imports.push(module);
        self
    }

    /// Adds providers, invokes and decorators.
    #[must_use]
    pub fn options(mut self, options: Options) -> Self {
        self.options = self.options.merge(options);
        self
    }

    /// Exports a key to the modules that import this one.
    #[must_use]
    pub fn export(mut self, export: Export) -> Self {
        self.exports.push(export.0);
        self
    }
}

/// A key a module exports.
#[derive(Clone, Copy, Debug)]
pub struct Export(pub(crate) Key);

impl Export {
    /// Exports the unnamed value of type `T` (which may be `dyn Trait`).
    pub fn of<T: ?Sized + 'static>() -> Self {
        Self(Key::of::<T>())
    }

    /// Exports the value of type `T` registered under `name`.
    pub fn named<T: ?Sized + 'static>(name: &'static str) -> Self {
        Self(Key::named::<T>(name))
    }
}
