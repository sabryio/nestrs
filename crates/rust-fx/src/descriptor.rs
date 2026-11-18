//! Descriptors: what a registered function looks like to the graph.
//!
//! [`constructor`], [`decorate`] and [`invoke`] wrap a function in a descriptor. The typed
//! builders ([`Provide`], [`Decorate`]) let you refine it (`named`, `in_group`, hooks,
//! `bind`); registering it erases the type into a [`Provider`], [`Decorator`] or
//! [`Invoke`] that the planner and runtime work with.

use std::future::Future;
use std::panic::Location;
use std::sync::Arc;

use crate::bind::Bound;
use crate::error::{BoxError, Result};
use crate::extract::{Args, Context, DepKind, DepSpec};
use crate::func::{Constructor, Invocation};
use crate::key::{Key, Origin};
use crate::lifecycle::HookOutcome;
use crate::value::{BoxFuture, Stored, store};

type Ctor<V> =
    Arc<dyn Fn(&mut Args, &Context) -> Result<BoxFuture<'static, std::result::Result<Arc<V>, BoxError>>> + Send + Sync>;

pub(crate) type BuildFn =
    Arc<dyn Fn(&mut Args, &Context) -> Result<BoxFuture<'static, std::result::Result<Stored, BoxError>>> + Send + Sync>;

pub(crate) type InvokeFn =
    Arc<dyn Fn(&mut Args, &Context) -> Result<BoxFuture<'static, std::result::Result<(), BoxError>>> + Send + Sync>;

/// A hook body, ready to be handed to the lifecycle.
type HookWork<T> = Box<dyn FnOnce() -> BoxFuture<'static, T> + Send>;

#[derive(Clone, Copy)]
enum Slot {
    Value(Option<&'static str>),
    Group(Option<&'static str>),
}

/// A constructor that is still being described.
///
/// Created by [`constructor`] or [`bind`](crate::bind()); refine it with [`named`](Self::named),
/// [`in_group`](Self::in_group), [`on_start`](Self::on_start) and friends, then hand it
/// to [`Options::constructor`](crate::Options::constructor).
pub struct Provide<V: ?Sized + 'static> {
    ctor: Ctor<V>,
    specs: Vec<DepSpec>,
    origin: Origin,
    slot: Slot,
}

/// Describes `function` as a constructor: its parameters are its dependencies and its
/// return type is what it provides. This is `fx.Provide`.
///
/// ```
/// use std::sync::Arc;
/// struct Greeter(String);
/// fn greeter() -> Arc<Greeter> { Arc::new(Greeter("hi".into())) }
/// let _ = fx::constructor(greeter);
/// ```
#[track_caller]
pub fn constructor<F, M>(function: F) -> Provide<<F as Constructor<M>>::Value>
where
    F: Constructor<M>,
{
    let origin = Origin::new(std::any::type_name::<F>(), Location::caller());
    let function = Arc::new(function);
    Provide {
        ctor: Arc::new(move |args, ctx| <F as Constructor<M>>::call(&function, args, ctx)),
        specs: F::specs(),
        origin,
        slot: Slot::Value(None),
    }
}

impl<V: ?Sized + Send + Sync + 'static> Provide<V> {
    /// Registers the value under `name`; consumers take it as
    /// [`Named<Tag, V>`](crate::Named).
    #[must_use]
    pub fn named(mut self, name: &'static str) -> Self {
        self.slot = Slot::Value(Some(name));
        self
    }

    /// Contributes the value to the unnamed group of `V` (see [`Group`](crate::Group)).
    #[must_use]
    pub fn in_group(mut self) -> Self {
        self.slot = Slot::Group(None);
        self
    }

    /// Contributes the value to the group called `name`.
    #[must_use]
    pub fn in_group_named(mut self, name: &'static str) -> Self {
        self.slot = Slot::Group(Some(name));
        self
    }

    /// Provides the value as the interface `I` *instead of* its concrete type (`fx.As`).
    ///
    /// Needs `V: Bound<I>`, which `#[fx::bind]` declares on the trait impl, so the cast
    /// is never written by hand and a missing declaration is a compile error.
    #[must_use]
    pub fn bind<I: ?Sized + Send + Sync + 'static>(self) -> Provide<I>
    where
        V: Bound<I>,
    {
        let inner = self.ctor;
        Provide {
            ctor: Arc::new(move |args, ctx| {
                let future = inner(args, ctx)?;
                Ok(Box::pin(async move { future.await.map(<V as Bound<I>>::upcast) }))
            }),
            specs: self.specs,
            origin: self.origin,
            slot: self.slot,
        }
    }

    /// Runs `hook` with the built value when the app starts.
    #[must_use]
    pub fn on_start<H, Fut>(self, hook: H) -> Self
    where
        H: Fn(Arc<V>) -> Fut + Send + Sync + 'static,
        Fut: Future + Send + 'static,
        Fut::Output: HookOutcome,
    {
        self.with_hook(hook, |life, work| life.on_start(work))
    }

    /// Runs `hook` with the built value when the app stops.
    #[must_use]
    pub fn on_stop<H, Fut>(self, hook: H) -> Self
    where
        H: Fn(Arc<V>) -> Fut + Send + Sync + 'static,
        Fut: Future + Send + 'static,
        Fut::Output: HookOutcome,
    {
        self.with_hook(hook, |life, work| life.on_stop(work))
    }

    fn with_hook<H, Fut>(self, hook: H, register: fn(&crate::Lifecycle, HookWork<Fut::Output>)) -> Self
    where
        H: Fn(Arc<V>) -> Fut + Send + Sync + 'static,
        Fut: Future + Send + 'static,
        Fut::Output: HookOutcome,
    {
        let inner = self.ctor;
        let hook = Arc::new(hook);
        Self {
            ctor: Arc::new(move |args, ctx| {
                let future = inner(args, ctx)?;
                let life = ctx.lifecycle.clone();
                let hook = hook.clone();
                Ok(Box::pin(async move {
                    let value = future.await?;
                    let target = value.clone();
                    register(
                        &life,
                        Box::new(move || {
                            let run: &H = &hook;
                            Box::pin(run(target))
                        }),
                    );
                    Ok(value)
                }))
            }),
            specs: self.specs,
            origin: self.origin,
            slot: self.slot,
        }
    }
}

/// A constructor with its type erased: what the planner and runtime see.
#[derive(Clone)]
pub struct Provider {
    pub(crate) key: Key,
    pub(crate) specs: Vec<DepSpec>,
    pub(crate) build: BuildFn,
    pub(crate) origin: Origin,
}

impl Provider {
    /// What this provider provides (or, for a group member, the group it joins).
    pub fn key(&self) -> Key {
        self.key
    }

    /// Where it was registered.
    pub fn origin(&self) -> Origin {
        self.origin
    }

    /// A provider that hands out `value`, depends on nothing and never fails.
    pub(crate) fn value<T: ?Sized + Send + Sync + 'static>(
        value: Arc<T>,
        key: Key,
        name: &'static str,
        location: &'static Location<'static>,
    ) -> Self {
        let stored = store(value);
        Self {
            key,
            specs: Vec::new(),
            build: Arc::new(move |_, _| Ok(Box::pin(std::future::ready(Ok(stored.clone()))))),
            origin: Origin::new(name, location),
        }
    }
}

impl<V: ?Sized + Send + Sync + 'static> From<Provide<V>> for Provider {
    fn from(provide: Provide<V>) -> Self {
        let key = match provide.slot {
            Slot::Value(None) => Key::of::<V>(),
            Slot::Value(Some(name)) => Key::named::<V>(name),
            Slot::Group(name) => Key::group::<V>(name),
        };
        let ctor = provide.ctor;
        Self {
            key,
            specs: provide.specs,
            build: Arc::new(move |args, ctx| {
                let future = ctor(args, ctx)?;
                Ok(Box::pin(async move { future.await.map(store::<V>) }))
            }),
            origin: provide.origin,
        }
    }
}

impl std::fmt::Debug for Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Provider({} by {})", self.key, self.origin)
    }
}

/// Anything that can be registered as a provider: a function, a [`Provide`] or a
/// [`Provider`].
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be registered as a provider",
    label = "not a valid constructor",
    note = "a constructor is a function whose parameters are `Arc<T>`, `Option<Arc<T>>`, `fx::Named`, `fx::Group`, `fx::Lifecycle` or `fx::Shutdowner`",
    note = "and whose return type is an `Arc<T>`, a type with `#[derive(fx::Component)]` / `#[fx::component]`, or a `Result` of either"
)]
pub trait IntoProvider<Marker> {
    /// Converts into the erased descriptor.
    fn into_provider(self) -> Provider;
}

#[doc(hidden)]
pub struct FromFunction<M>(std::marker::PhantomData<M>);
#[doc(hidden)]
pub struct Described;

impl<F, M> IntoProvider<FromFunction<M>> for F
where
    F: Constructor<M>,
{
    #[track_caller]
    fn into_provider(self) -> Provider {
        constructor(self).into()
    }
}

impl<V: ?Sized + Send + Sync + 'static> IntoProvider<Described> for Provide<V> {
    fn into_provider(self) -> Provider {
        self.into()
    }
}

impl IntoProvider<Described> for Provider {
    fn into_provider(self) -> Provider {
        self
    }
}

/// A decorator that is still being described. See [`decorate`].
pub struct Decorate<V: ?Sized + 'static> {
    ctor: Ctor<V>,
    specs: Vec<DepSpec>,
    origin: Origin,
    name: Option<&'static str>,
}

/// Describes `function` as a decorator: it takes a `V` (plus any other dependencies)
/// and returns the `V` consumers should see. This is `fx.Decorate`.
///
/// Within a module it affects that module's consumers; at the app root it affects
/// everyone. It always receives the *undecorated* value as its `V` argument.
#[track_caller]
pub fn decorate<F, M>(function: F) -> Decorate<<F as Constructor<M>>::Value>
where
    F: Constructor<M>,
{
    let origin = Origin::new(std::any::type_name::<F>(), Location::caller());
    let function = Arc::new(function);
    Decorate {
        ctor: Arc::new(move |args, ctx| <F as Constructor<M>>::call(&function, args, ctx)),
        specs: F::specs(),
        origin,
        name: None,
    }
}

impl<V: ?Sized + Send + Sync + 'static> Decorate<V> {
    /// Decorates the value registered under `name`.
    #[must_use]
    pub fn named(mut self, name: &'static str) -> Self {
        self.name = Some(name);
        self
    }
}

/// A decorator with its type erased.
#[derive(Clone)]
pub struct Decorator {
    pub(crate) key: Key,
    pub(crate) specs: Vec<DepSpec>,
    pub(crate) build: BuildFn,
    pub(crate) origin: Origin,
}

impl Decorator {
    pub(crate) fn takes_target(&self) -> bool {
        self.specs
            .iter()
            .any(|spec| spec.key == self.key && spec.kind == DepKind::Required)
    }
}

impl<V: ?Sized + Send + Sync + 'static> From<Decorate<V>> for Decorator {
    fn from(decorate: Decorate<V>) -> Self {
        let key = match decorate.name {
            Some(name) => Key::named::<V>(name),
            None => Key::of::<V>(),
        };
        let ctor = decorate.ctor;
        Self {
            key,
            specs: decorate.specs,
            build: Arc::new(move |args, ctx| {
                let future = ctor(args, ctx)?;
                Ok(Box::pin(async move { future.await.map(store::<V>) }))
            }),
            origin: decorate.origin,
        }
    }
}

/// Anything that can be registered as a decorator.
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be registered as a decorator",
    label = "not a valid decorator",
    note = "a constructor is a function whose parameters are `Arc<T>`, `Option<Arc<T>>`, `fx::Named`, `fx::Group`, `fx::Lifecycle` or `fx::Shutdowner`",
    note = "and whose return type is an `Arc<T>`, a type with `#[derive(fx::Component)]` / `#[fx::component]`, or a `Result` of either"
)]
pub trait IntoDecorator<Marker> {
    /// Converts into the erased descriptor.
    fn into_decorator(self) -> Decorator;
}

impl<F, M> IntoDecorator<FromFunction<M>> for F
where
    F: Constructor<M>,
{
    #[track_caller]
    fn into_decorator(self) -> Decorator {
        decorate(self).into()
    }
}

impl<V: ?Sized + Send + Sync + 'static> IntoDecorator<Described> for Decorate<V> {
    fn into_decorator(self) -> Decorator {
        self.into()
    }
}

impl IntoDecorator<Described> for Decorator {
    fn into_decorator(self) -> Decorator {
        self
    }
}

/// A function to call once at startup, with its type erased. See [`invoke`].
#[derive(Clone)]
pub struct Invoke {
    pub(crate) specs: Vec<DepSpec>,
    pub(crate) build: InvokeFn,
    pub(crate) origin: Origin,
}

/// Describes `function` as an invoke: it runs once at startup, after everything it
/// depends on has been built, and its result is discarded. This is `fx.Invoke`.
///
/// Invokes run in the order they were registered. Only what an invoke (transitively)
/// needs is constructed: providers nobody asks for are never built.
#[track_caller]
pub fn invoke<F, M>(function: F) -> Invoke
where
    F: Invocation<M>,
{
    let origin = Origin::new(std::any::type_name::<F>(), Location::caller());
    let function = Arc::new(function);
    Invoke {
        specs: F::specs(),
        build: Arc::new(move |args, ctx| <F as Invocation<M>>::call(&function, args, ctx)),
        origin,
    }
}

/// Anything that can be registered as an invoke.
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be registered as an invoke",
    label = "not a valid invoke function",
    note = "an invoke is a function whose parameters are `Arc<T>`, `Option<Arc<T>>`, `fx::Named`, `fx::Group`, `fx::Lifecycle` or `fx::Shutdowner`",
    note = "and which returns `()` or a `Result<(), E>`"
)]
pub trait IntoInvoke<Marker> {
    /// Converts into the erased descriptor.
    fn into_invoke(self) -> Invoke;
}

impl<F, M> IntoInvoke<FromFunction<M>> for F
where
    F: Invocation<M>,
{
    #[track_caller]
    fn into_invoke(self) -> Invoke {
        invoke(self)
    }
}

impl IntoInvoke<Described> for Invoke {
    fn into_invoke(self) -> Invoke {
        self
    }
}
