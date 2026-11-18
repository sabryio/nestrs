//! Interface bindings: providing a concrete component as a trait object (`fx.As`).
//!
//! A binding has two halves. The *declaration* says "this type may be used as that trait"
//! and is the [`Bound`] impl that `#[fx::bind]` writes on the trait impl itself. The
//! *registration* makes it so inside a module or app, and happens on its own whenever a
//! constructor of the plain (unnamed, ungrouped) value of that type is registered
//! ([`Options::constructor`]), so providers never spell the cast or the `dyn` type.

use std::any::TypeId;
use std::sync::Arc;

use crate::descriptor::{Provide, constructor};
use crate::options::Options;

/// Declares that `Self` can be shared as the interface `I` (usually `dyn Trait`).
///
/// Implemented by `#[fx::bind]`; implement it by hand only when the attribute cannot be
/// used (for example for a trait you do not own, in the crate that owns the type).
///
/// ```
/// use std::sync::Arc;
/// trait Clock: Send + Sync { fn now(&self) -> u64; }
/// struct System;
/// impl Clock for System { fn now(&self) -> u64 { 7 } }
/// impl fx::Bound<dyn Clock> for System {
///     fn upcast(this: Arc<Self>) -> Arc<dyn Clock> { this }
/// }
/// ```
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not declared as an implementation of the interface `{I}`",
    label = "missing `#[fx::bind]`",
    note = "put `#[fx::bind]` on the `impl Trait for {Self}` block that implements the interface"
)]
pub trait Bound<I: ?Sized>: Send + Sync + Sized + 'static {
    /// The (free) conversion from the concrete value to the interface.
    fn upcast(this: Arc<Self>) -> Arc<I>;
}

/// The constructor behind a binding: takes the concrete `C`, hands out `I`.
fn upcast<I, C>(concrete: Arc<C>) -> Arc<I>
where
    I: ?Sized + Send + Sync + 'static,
    C: Bound<I>,
{
    C::upcast(concrete)
}

/// A provider of the interface `I` built from the component `C` (`fx.As`, additive).
///
/// `C` itself stays provided; export only `dyn Trait` from a module to publish just the
/// interface.
///
/// ```
/// use std::sync::Arc;
/// trait Clock: Send + Sync { fn now(&self) -> u64; }
/// struct System;
/// impl Clock for System { fn now(&self) -> u64 { 7 } }
/// impl fx::Bound<dyn Clock> for System {
///     fn upcast(this: Arc<Self>) -> Arc<dyn Clock> { this }
/// }
/// impl fx::Component for System {}
///
/// fn use_it(clock: Arc<dyn Clock>) { assert_eq!(clock.now(), 7); }
///
/// let app = fx::App::new()
///     .constructor(|| System)
///     .constructor(fx::bind::<dyn Clock, System>())
///     .invoke(use_it);
/// app.validate().unwrap();
/// ```
#[track_caller]
pub fn bind<I, C>() -> Provide<I>
where
    I: ?Sized + Send + Sync + 'static,
    C: Bound<I>,
{
    constructor(upcast::<I, C>)
}

/// Where the bound interface value is placed, mirroring the constructor options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[doc(hidden)]
pub enum Placement {
    /// The plain, unnamed value.
    Value,
    /// A named value.
    Named(&'static str),
    /// The unnamed group of the interface.
    Group,
    /// A named group of the interface.
    GroupNamed(&'static str),
}

/// A binding recorded by `#[fx::bind]` at link time and applied when a constructor of its
/// component is registered. Not part of the public API.
#[doc(hidden)]
pub struct Binding {
    component: fn() -> TypeId,
    interface: fn() -> &'static str,
    placement: Placement,
    apply: fn(Options, Placement) -> Options,
}

impl Binding {
    /// Builds the record for `C` bound as `I`.
    pub const fn new<I, C>(placement: Placement) -> Self
    where
        I: ?Sized + Send + Sync + 'static,
        C: Bound<I>,
    {
        Self {
            component: TypeId::of::<C>,
            interface: std::any::type_name::<I>,
            placement,
            apply: apply::<I, C>,
        }
    }
}

fn apply<I, C>(options: Options, placement: Placement) -> Options
where
    I: ?Sized + Send + Sync + 'static,
    C: Bound<I>,
{
    let provide = bind::<I, C>();
    options.constructor(match placement {
        Placement::Value => provide,
        Placement::Named(name) => provide.named(name),
        Placement::Group => provide.in_group(),
        Placement::GroupNamed(name) => provide.in_group_named(name),
    })
}

inventory::collect!(Binding);

/// Registers every binding declared for the type `target`, in a stable order.
pub(crate) fn apply_declared(mut options: Options, target: TypeId) -> Options {
    let mut declared: Vec<&Binding> = inventory::iter::<Binding>()
        .filter(|binding| (binding.component)() == target)
        .collect();
    declared.sort_by_key(|binding| ((binding.interface)(), format!("{:?}", binding.placement)));
    for binding in declared {
        options = (binding.apply)(options, binding.placement);
    }
    options
}
