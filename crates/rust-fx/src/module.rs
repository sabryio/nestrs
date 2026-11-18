//! Modules: named, reusable bundles of options with a private scope.

use std::any::TypeId;

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

/// A lazy reference to a module, so that import cycles can be reported instead of
/// recursing forever.
#[derive(Clone, Copy)]
pub struct ModuleRef {
    pub(crate) id: TypeId,
    pub(crate) name: &'static str,
    pub(crate) definition: fn() -> ModuleDef,
}

impl ModuleRef {
    /// A reference to module `M`.
    pub fn of<M: Module>() -> Self {
        Self {
            id: TypeId::of::<M>(),
            name: std::any::type_name::<M>(),
            definition: M::definition,
        }
    }
}

impl std::fmt::Debug for ModuleRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ModuleRef({})", self.name)
    }
}

/// What a module is made of.
pub struct ModuleDef {
    pub(crate) name: &'static str,
    pub(crate) imports: Vec<ModuleRef>,
    pub(crate) options: Options,
    pub(crate) exports: Vec<Key>,
}

impl ModuleDef {
    /// An empty module called `name`.
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            imports: Vec::new(),
            options: Options::new(),
            exports: Vec::new(),
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
