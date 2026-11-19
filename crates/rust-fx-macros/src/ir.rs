//! The intermediate representation: what a macro *means*, independent of how it was written
//! and of the Rust it will become.
//!
//! Everything here is valid and fully resolved. Analysis produces it; code generation only
//! reads it.

use syn::{Expr, Ident, ItemImpl, LitStr, Path, Type};

/// Where a registered value lives.
#[derive(Debug, Clone)]
pub(crate) enum Placement {
    /// The plain, unnamed value.
    Value,
    /// Under a name.
    Named(LitStr),
    /// In the unnamed group (`None`) or a named group.
    Group(Option<LitStr>),
}

/// An associated function registered as a constructor.
#[derive(Debug, Clone)]
pub(crate) struct Constructor {
    pub(crate) method: Ident,
    pub(crate) placement: Placement,
}

/// When a lifecycle hook runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    Start,
    Stop,
}

/// A `&self` method run when the built value starts or stops.
#[derive(Debug, Clone)]
pub(crate) struct Hook {
    pub(crate) method: Ident,
    pub(crate) phase: Phase,
    pub(crate) is_async: bool,
}

/// Everything an `#[fx::component]` block contributes.
pub(crate) struct Component {
    /// The block itself, with our attributes removed.
    pub(crate) item: ItemImpl,
    pub(crate) constructors: Vec<Constructor>,
    pub(crate) hooks: Vec<Hook>,
    pub(crate) invokes: Vec<Ident>,
    pub(crate) decorators: Vec<Ident>,
}

/// A trait implementation declared usable as an interface (`#[fx::bind]`).
pub(crate) struct Binding {
    pub(crate) item: ItemImpl,
    /// The implementing type.
    pub(crate) component: Type,
    /// The trait, as written in the `impl` header.
    pub(crate) interface: Path,
    pub(crate) placement: Placement,
}

/// One entry of a module's `providers` list.
pub(crate) enum Provider {
    /// A component type: registers everything its `impl` declares.
    Component(Path),
    /// Any other expression: one constructor.
    Constructor(Expr),
}

/// One entry of a module's `exports` list.
pub(crate) struct Export {
    pub(crate) ty: Type,
    pub(crate) name: Option<LitStr>,
}

/// A module declaration.
pub(crate) struct Module {
    pub(crate) item: syn::ItemStruct,
    pub(crate) name: String,
    pub(crate) imports: Vec<Type>,
    pub(crate) providers: Vec<Provider>,
    pub(crate) invokes: Vec<Expr>,
    pub(crate) decorators: Vec<Expr>,
    pub(crate) exports: Vec<Export>,
}

/// A struct whose fields are all extracted dependencies (`#[derive(fx::Params)]`).
pub(crate) struct Params {
    pub(crate) name: Ident,
    pub(crate) shape: ParamsShape,
}

pub(crate) enum ParamsShape {
    Named(Vec<(Ident, Type)>),
    Tuple(Vec<Type>),
    Unit,
}

/// A type that may be returned by value from constructors (`#[derive(fx::Component)]`).
pub(crate) struct ComponentMarker {
    pub(crate) input: syn::DeriveInput,
}

/// A trait declared as an application port (`#[fx::port]`).
pub(crate) struct Port {
    /// The trait, with `Send + Sync` supertraits ensured.
    pub(crate) item: syn::ItemTrait,
    pub(crate) vis: syn::Visibility,
    pub(crate) trait_ident: Ident,
    /// The generated `type <alias> = dyn <Trait>`.
    pub(crate) alias: Ident,
}

/// An async entry point (`#[fx::main]`).
pub(crate) struct Entry {
    pub(crate) item: syn::ItemFn,
    /// Runtime options, passed through unchanged.
    pub(crate) options: Vec<syn::Meta>,
}
