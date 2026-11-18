//! Function parameters that the graph knows how to fill.
//!
//! A constructor, invoke or decorator is a plain function. Each parameter type
//! implements [`Extract`], which says what it needs from the graph and how to take it
//! out of the resolved arguments. The built-in extractors are:
//!
//! | Parameter | Meaning |
//! |---|---|
//! | `Arc<T>` | the provided `T` (`T` may be `dyn Trait`) |
//! | `Option<Arc<T>>` ([`Optional`]) | `None` if nobody provides `T` |
//! | [`Named<G, T>`] | the `T` registered under the name of tag `G` |
//! | [`Group<T>`] / [`Group<T, G>`] | every `T` contributed to the group |
//! | [`Lifecycle`] | the hook registry |
//! | [`Shutdowner`] | the shutdown handle |
//!
//! `#[derive(fx::Params)]` bundles several of them into one struct (fx's `fx.In`).

use std::marker::PhantomData;
use std::ops::Deref;
use std::sync::Arc;

use crate::error::{Error, Result};
use crate::key::Key;
use crate::lifecycle::Lifecycle;
use crate::shutdown::Shutdowner;
use crate::value::{Stored, load};

/// How a dependency is looked up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepKind {
    /// Must be provided.
    Required,
    /// `None` when nobody provides it.
    Optional,
    /// Every contribution to a group.
    Group,
}

/// One thing a function needs from the graph.
#[derive(Debug, Clone, Copy)]
pub struct DepSpec {
    /// What to look up.
    pub key: Key,
    /// How to look it up.
    pub kind: DepKind,
}

/// A resolved argument, as handed to [`Extract::extract`].
#[derive(Clone)]
pub enum Resolved {
    /// A required value.
    One(Stored),
    /// An optional value.
    Maybe(Option<Stored>),
    /// A group's members, in registration order.
    Many(Vec<Stored>),
}

/// The resolved arguments of one call, consumed front to back.
pub struct Args {
    items: std::vec::IntoIter<Resolved>,
}

impl Args {
    pub(crate) fn new(items: Vec<Resolved>) -> Self {
        Self {
            items: items.into_iter(),
        }
    }

    /// Takes the next resolved argument.
    pub fn take(&mut self) -> Result<Resolved> {
        self.items
            .next()
            .ok_or_else(|| Error::internal("a function asked for more arguments than the graph resolved"))
    }
}

/// What the runtime offers a function besides graph values.
#[derive(Clone)]
pub struct Context {
    /// Where hooks are registered.
    pub lifecycle: Lifecycle,
    /// How to request a shutdown.
    pub shutdowner: Shutdowner,
}

/// A type that can be a parameter of a constructor, invoke or decorator.
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be a parameter of a constructor, invoke or decorator",
    label = "not an injectable parameter",
    note = "use `Arc<T>`, `Option<Arc<T>>`, `fx::Named<Tag, T>`, `fx::Group<T>`, `fx::Lifecycle` or `fx::Shutdowner`",
    note = "or a struct with `#[derive(fx::Params)]`"
)]
pub trait Extract: Sized + Send + 'static {
    /// Appends what this parameter needs from the graph (nothing for context handles).
    fn specs(out: &mut Vec<DepSpec>);

    /// Takes this parameter out of the resolved arguments.
    ///
    /// # Errors
    ///
    /// [`Error::Internal`] if the arguments do not have the shape [`Extract::specs`]
    /// announced. That is a framework bug, not a user error.
    fn extract(args: &mut Args, ctx: &Context) -> Result<Self>;
}

fn shape(expected: &str) -> Error {
    Error::internal(format!("expected a {expected} argument"))
}

fn mismatch<T: ?Sized>() -> Error {
    Error::internal(format!(
        "resolved value is not `{}`",
        crate::key::shorten(std::any::type_name::<T>())
    ))
}

impl<T: ?Sized + Send + Sync + 'static> Extract for Arc<T> {
    fn specs(out: &mut Vec<DepSpec>) {
        out.push(DepSpec {
            key: Key::of::<T>(),
            kind: DepKind::Required,
        });
    }

    fn extract(args: &mut Args, _: &Context) -> Result<Self> {
        match args.take()? {
            Resolved::One(stored) => load::<T>(&stored).ok_or_else(mismatch::<T>),
            _ => Err(shape("single")),
        }
    }
}

/// `None` when nobody provides `T`: fx's `optional:"true"`.
pub type Optional<T> = Option<Arc<T>>;

impl<T: ?Sized + Send + Sync + 'static> Extract for Option<Arc<T>> {
    fn specs(out: &mut Vec<DepSpec>) {
        out.push(DepSpec {
            key: Key::of::<T>(),
            kind: DepKind::Optional,
        });
    }

    fn extract(args: &mut Args, _: &Context) -> Result<Self> {
        match args.take()? {
            Resolved::Maybe(None) => Ok(None),
            Resolved::Maybe(Some(stored)) => load::<T>(&stored).map(Some).ok_or_else(mismatch::<T>),
            _ => Err(shape("optional")),
        }
    }
}

impl Extract for Lifecycle {
    fn specs(_: &mut Vec<DepSpec>) {}

    fn extract(_: &mut Args, ctx: &Context) -> Result<Self> {
        Ok(ctx.lifecycle.clone())
    }
}

impl Extract for Shutdowner {
    fn specs(_: &mut Vec<DepSpec>) {}

    fn extract(_: &mut Args, ctx: &Context) -> Result<Self> {
        Ok(ctx.shutdowner.clone())
    }
}

/// A name for [`Named`] values and named groups. Declare one with [`tag!`](crate::tag).
pub trait Tag: 'static {
    /// The name values carry in the graph.
    const NAME: &'static str;
}

/// Names a group: [`Unnamed`] (the group of all values of a type) or any [`Tag`].
pub trait GroupName: 'static {
    /// The group's name, `None` for the unnamed group.
    const NAME: Option<&'static str>;
}

/// The default group of a type.
#[derive(Debug, Clone, Copy)]
pub struct Unnamed;

impl GroupName for Unnamed {
    const NAME: Option<&'static str> = None;
}

impl<G: Tag> GroupName for G {
    const NAME: Option<&'static str> = Some(<G as Tag>::NAME);
}

/// The value of type `T` registered under the name of tag `G`: fx's `name:"..."`.
///
/// ```
/// use std::sync::Arc;
/// fx::tag!(Primary = "primary");
/// fn uses(db: fx::Named<Primary, String>) { let _ = db.len(); }
/// let app = fx::App::new()
///     .constructor(fx::constructor(|| Arc::new("db".to_string())).named("primary"))
///     .invoke(uses);
/// app.validate().unwrap();
/// ```
pub struct Named<G, T: ?Sized>(Arc<T>, PhantomData<fn() -> G>);

impl<G, T: ?Sized> Named<G, T> {
    /// The inner handle.
    pub fn into_inner(self) -> Arc<T> {
        self.0
    }
}

impl<G, T: ?Sized> Deref for Named<G, T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

impl<G, T: ?Sized> Clone for Named<G, T> {
    fn clone(&self) -> Self {
        Self(self.0.clone(), PhantomData)
    }
}

impl<G: Tag, T: ?Sized + Send + Sync + 'static> Extract for Named<G, T> {
    fn specs(out: &mut Vec<DepSpec>) {
        out.push(DepSpec {
            key: Key::named::<T>(G::NAME),
            kind: DepKind::Required,
        });
    }

    fn extract(args: &mut Args, _: &Context) -> Result<Self> {
        match args.take()? {
            Resolved::One(stored) => load::<T>(&stored)
                .map(|value| Self(value, PhantomData))
                .ok_or_else(mismatch::<T>),
            _ => Err(shape("single")),
        }
    }
}

/// Every `T` contributed to a group, in registration order: fx's `group:"..."`.
///
/// Members are contributed with [`Provide::in_group`](crate::Provide::in_group).
/// Groups are app-wide: they collect contributions from every module.
pub struct Group<T: ?Sized, G = Unnamed>(Vec<Arc<T>>, PhantomData<fn() -> G>);

impl<T: ?Sized, G> Group<T, G> {
    /// The members as a vector.
    pub fn into_vec(self) -> Vec<Arc<T>> {
        self.0
    }
}

impl<T: ?Sized, G> Deref for Group<T, G> {
    type Target = [Arc<T>];

    fn deref(&self) -> &[Arc<T>] {
        &self.0
    }
}

impl<T: ?Sized, G> IntoIterator for Group<T, G> {
    type Item = Arc<T>;
    type IntoIter = std::vec::IntoIter<Arc<T>>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a, T: ?Sized, G> IntoIterator for &'a Group<T, G> {
    type Item = &'a Arc<T>;
    type IntoIter = std::slice::Iter<'a, Arc<T>>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl<T: ?Sized, G> Clone for Group<T, G> {
    fn clone(&self) -> Self {
        Self(self.0.clone(), PhantomData)
    }
}

impl<T: ?Sized + Send + Sync + 'static, G: GroupName> Extract for Group<T, G> {
    fn specs(out: &mut Vec<DepSpec>) {
        out.push(DepSpec {
            key: Key::group::<T>(G::NAME),
            kind: DepKind::Group,
        });
    }

    fn extract(args: &mut Args, _: &Context) -> Result<Self> {
        match args.take()? {
            Resolved::Many(stored) => stored
                .iter()
                .map(|s| load::<T>(s).ok_or_else(mismatch::<T>))
                .collect::<Result<Vec<_>>>()
                .map(|members| Self(members, PhantomData)),
            _ => Err(shape("group")),
        }
    }
}
