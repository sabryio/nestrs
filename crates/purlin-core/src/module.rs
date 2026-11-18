use std::any::TypeId;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::{ControllerEntry, Provider, Token};

/// A unit of composition: a bundle of providers and controllers with an explicit
/// public surface.
///
/// Usually produced by `#[module(..)]`; implementing it by hand is equally valid and
/// is how third-party crates can ship modules without depending on the macros.
///
/// A `Module` is fixed at compile time. For modules that take configuration
/// (`DatabaseModule::for_root(config)`), build a [`ModuleDef`] at runtime and turn it
/// into a [`ModuleRef`] with [`ModuleDef::into_ref`].
pub trait Module: 'static {
    /// Describes the module. Called once per bootstrap.
    fn definition() -> ModuleDef;
}

/// What makes two imports "the same module".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ModuleId {
    /// A [`Module`] type: every `ModuleRef::of::<M>()` is the same module.
    Type(TypeId),
    /// A configured instance: every clone of one `ModuleRef` is the same module,
    /// two separately built ones are not.
    Instance(u64),
}

#[derive(Clone)]
enum Source {
    Static(fn() -> ModuleDef),
    Dynamic(Arc<ModuleDef>),
}

/// A reference to a module, used in `imports`.
///
/// Importing the same module from several places builds it once:
///
/// - for a [`Module`] type, identity is the type;
/// - for a configured module ([`ModuleDef::into_ref`]), identity is the instance, so
///   clone one `ModuleRef` to share it. Two separate `for_root(..)` calls produce two
///   instances, and if both provide the same type bootstrap reports a
///   [`DuplicateProvider`](crate::DiError::DuplicateProvider) instead of silently
///   picking a configuration. Configure once at the root and mark the module
///   [`global`](ModuleDef::global).
#[derive(Clone)]
pub struct ModuleRef {
    id: ModuleId,
    source: Source,
}

impl ModuleRef {
    /// References the module `M`.
    pub fn of<M: Module>() -> Self {
        Self {
            id: ModuleId::Type(TypeId::of::<M>()),
            source: Source::Static(M::definition),
        }
    }

    pub(crate) fn id(&self) -> ModuleId {
        self.id
    }

    pub(crate) fn define(&self) -> ModuleDef {
        match &self.source {
            Source::Static(definition) => definition(),
            Source::Dynamic(definition) => ModuleDef::clone(definition),
        }
    }
}

/// The full description of a module.
///
/// - `imports`: modules whose exports this module may depend on.
/// - `providers`: singletons this module owns.
/// - `controllers`: transport entry points; they are providers too, so they can
///   depend on anything the module can see.
/// - `exports`: the subset of providers (or re-exported imports) that importers see.
/// - `global`: its exports are visible everywhere without importing it.
#[derive(Clone)]
pub struct ModuleDef {
    pub(crate) name: &'static str,
    pub(crate) global: bool,
    pub(crate) imports: Vec<ModuleRef>,
    pub(crate) providers: Vec<Provider>,
    pub(crate) controllers: Vec<ControllerEntry>,
    pub(crate) exports: Vec<Token>,
}

impl ModuleDef {
    /// Starts a definition. `name` shows up in diagnostics.
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            global: false,
            imports: Vec::new(),
            providers: Vec::new(),
            controllers: Vec::new(),
            exports: Vec::new(),
        }
    }

    /// Makes this module's exports visible to every module in the graph, without an
    /// explicit import (think configuration, logging, a database pool).
    ///
    /// The module must still be part of the graph: import it once, typically from
    /// the root.
    pub fn global(mut self) -> Self {
        self.global = true;
        self
    }

    /// Adds imported modules.
    pub fn imports(mut self, modules: impl IntoIterator<Item = ModuleRef>) -> Self {
        self.imports.extend(modules);
        self
    }

    /// Adds providers owned by this module.
    pub fn providers(mut self, providers: impl IntoIterator<Item = Provider>) -> Self {
        self.providers.extend(providers);
        self
    }

    /// Adds controllers owned by this module.
    pub fn controllers(mut self, controllers: impl IntoIterator<Item = ControllerEntry>) -> Self {
        self.controllers.extend(controllers);
        self
    }

    /// Adds exported tokens.
    pub fn exports(mut self, tokens: impl IntoIterator<Item = Token>) -> Self {
        self.exports.extend(tokens);
        self
    }

    /// Turns a runtime-built definition into an importable reference: the building
    /// block of configurable modules.
    ///
    /// ```
    /// # use purlin_core::{ModuleDef, ModuleRef, Provider, Token};
    /// struct DbConfig { url: String }
    ///
    /// struct DatabaseModule;
    /// impl DatabaseModule {
    ///     fn for_root(config: DbConfig) -> ModuleRef {
    ///         ModuleDef::new("DatabaseModule")
    ///             .global()
    ///             .providers([Provider::value(config)])
    ///             .exports([Token::of::<DbConfig>()])
    ///             .into_ref()
    ///     }
    /// }
    /// # let _ = DatabaseModule::for_root(DbConfig { url: String::new() });
    /// ```
    pub fn into_ref(self) -> ModuleRef {
        static NEXT_INSTANCE: AtomicU64 = AtomicU64::new(0);
        ModuleRef {
            id: ModuleId::Instance(NEXT_INSTANCE.fetch_add(1, Ordering::Relaxed)),
            source: Source::Dynamic(Arc::new(self)),
        }
    }
}
