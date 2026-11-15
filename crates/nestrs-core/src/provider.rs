use std::future::Future;
use std::marker::PhantomData;
use std::ops::Deref;
use std::pin::Pin;
use std::sync::Arc;

use crate::container::Stored;
use crate::lifecycle::Hook;
use crate::{Container, DiError, Token};
use crate::{Lifetime, OnModuleDestroy, OnModuleInit};

/// A heap-allocated, sendable future; the shape factories return.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A type the container knows how to construct.
///
/// Normally derived with `#[injectable]`; implement by hand for anything the macro
/// cannot express (async setup, conditional wiring).
///
/// `dependencies` must list every type `build` resolves from the container. The
/// bootstrap uses it to order construction and to reject unreachable or missing
/// dependencies before anything is built.
pub trait Injectable: Sized + Send + Sync + 'static {
    /// The types this provider resolves while building.
    fn dependencies() -> Vec<Token>;

    /// Builds the provider. Dependencies are guaranteed to be in the container
    /// already; they can be resolved with [`Container::resolve`].
    fn build(container: &Container) -> impl Future<Output = Result<Self, DiError>> + Send;

    /// The providers a module registers when it lists this type.
    ///
    /// The default is the type itself as a singleton. `#[injectable]` overrides it
    /// to attach lifecycle hooks and to bind a port (`provides = ..`).
    fn register() -> Vec<Provider> {
        vec![Provider::of::<Self>()]
    }
}

/// Shared (`Arc`) so that providers, and therefore whole module definitions, are
/// cheap to clone; that is what lets one configured module be imported from several
/// places.
type Factory = Arc<dyn for<'a> Fn(&'a Container) -> BoxFuture<'a, Result<Stored, DiError>> + Send + Sync>;

/// Pins down the higher-ranked closure signature so call sites can use plain
/// `|c| Box::pin(async move { .. })` closures.
fn boxed<F>(factory: F) -> Factory
where
    F: for<'a> Fn(&'a Container) -> BoxFuture<'a, Result<Stored, DiError>> + Send + Sync + 'static,
{
    Arc::new(factory)
}

fn resolving_hook<T>(hook: impl Fn(Arc<T>) -> BoxFuture<'static, Result<(), DiError>> + Send + Sync + 'static) -> Hook
where
    T: ?Sized + Send + Sync + 'static,
{
    Arc::new(move |container| match container.resolve::<T>() {
        Ok(instance) => hook(instance),
        Err(error) => Box::pin(async move { Err(error) }),
    })
}

fn store<T>(value: Arc<T>) -> Stored
where
    T: ?Sized + Send + Sync + 'static,
{
    Arc::new(value)
}

/// Every implementation contributed to the interface `I`, in registration order
/// (modules in dependency order, then declaration order within a module).
///
/// This is the multi-binding counterpart of a single `Arc<dyn Port>`: depend on
/// `Arc<Multi<dyn HealthIndicator>>` and receive all of them. See
/// [`Provider::contribute`] and [`Provider::multi`].
pub struct Multi<I: ?Sized>(Vec<Arc<I>>);

impl<I: ?Sized> Multi<I> {
    /// A set made of `items`; handy for [`Provider::value`] overrides in tests.
    pub fn new(items: Vec<Arc<I>>) -> Self {
        Self(items)
    }
}

impl<I: ?Sized> Deref for Multi<I> {
    type Target = [Arc<I>];

    fn deref(&self) -> &[Arc<I>] {
        &self.0
    }
}

impl<I: ?Sized + 'static> std::fmt::Debug for Multi<I> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Multi({} x {})", self.0.len(), Token::of::<I>())
    }
}

/// Builds the [`Multi`] for one interface from its members' tokens. Generic code
/// that knows `I` is captured here so the graph can stay type-erased.
type Assemble = fn(&Container, &[Token]) -> Result<Stored, DiError>;

fn assemble<I: ?Sized + Send + Sync + 'static>(container: &Container, members: &[Token]) -> Result<Stored, DiError> {
    let mut items = Vec::with_capacity(members.len());
    for member in members {
        let item = container
            .get_raw(*member)
            .and_then(|stored| stored.downcast_ref::<Arc<I>>().cloned())
            .ok_or(DiError::NotFound { requested: *member })?;
        items.push(item);
    }
    Ok(store::<Multi<I>>(Arc::new(Multi(items))))
}

/// The identity of "`T` as a member of `I`": distinct from `T` itself and from `I`.
struct Member<I: ?Sized, T>(PhantomData<fn() -> Pair<I, T>>);

/// Keeps [`Member`]'s marker simple; never constructed.
struct Pair<I: ?Sized, T>(Box<I>, T);

/// How a provider takes part in a multi-binding.
#[derive(Clone)]
pub(crate) struct MultiRole {
    pub(crate) target: Token,
    pub(crate) assemble: Assemble,
    /// A member adds itself to the set; a declaration only makes sure the set exists.
    pub(crate) member: bool,
}

/// A recipe for one singleton: which type it provides, what it needs first, and
/// how to build it.
///
/// Cloning a provider clones the recipe, not the singleton: each bootstrap builds
/// its own instance.
#[derive(Clone)]
pub struct Provider {
    token: Token,
    dependencies: Vec<Token>,
    factory: Factory,
    init: Vec<Hook>,
    destroy: Vec<Hook>,
    lifetime: Lifetime,
    /// Dependencies resolved per request (by extractors), exempt from the lifetime rule.
    per_request: Vec<Token>,
    multi: Option<MultiRole>,
}

impl Provider {
    fn new(token: Token, dependencies: Vec<Token>, factory: Factory) -> Self {
        Self {
            token,
            dependencies,
            factory,
            init: Vec::new(),
            destroy: Vec::new(),
            lifetime: Lifetime::Singleton,
            per_request: Vec::new(),
            multi: None,
        }
    }

    /// Makes this provider request-scoped: built lazily, once per request.
    ///
    /// Resolve it with [`RequestScope::resolve`](crate::RequestScope::resolve) (in
    /// HTTP handlers, the `Inject<T>` extractor). Lifecycle hooks only run for
    /// singletons.
    pub fn request(mut self) -> Self {
        self.lifetime = Lifetime::Request;
        self
    }

    /// Makes this provider transient: built anew every time it is resolved.
    pub fn transient(mut self) -> Self {
        self.lifetime = Lifetime::Transient;
        self
    }

    /// A request-scoped `T` whose value the transport supplies for every request
    /// (see [`RequestScope::seed`](crate::RequestScope::seed)). Declaring it lets
    /// other request-scoped providers depend on it.
    pub fn seeded<T: Send + Sync + 'static>() -> Self {
        Self::new(
            Token::of::<T>(),
            Vec::new(),
            boxed(|_| {
                Box::pin(async {
                    Err(DiError::custom(format!(
                        "`{}` is seeded per request, but nothing seeded it",
                        Token::of::<T>()
                    )))
                })
            }),
        )
        .request()
    }

    /// Declares dependencies that a *singleton* (a controller) resolves per request
    /// at call time. They must exist and be visible like any dependency, but may be
    /// request-scoped or transient.
    pub fn with_request_dependencies(mut self, tokens: impl IntoIterator<Item = Token>) -> Self {
        for token in tokens {
            self.dependencies.push(token);
            self.per_request.push(token);
        }
        self
    }

    /// This provider's lifetime.
    pub fn lifetime(&self) -> Lifetime {
        self.lifetime
    }

    pub(crate) fn is_per_request(&self, token: &Token) -> bool {
        self.per_request.contains(token)
    }

    /// Provides `T` by calling its [`Injectable`] implementation.
    pub fn of<T: Injectable>() -> Self {
        Self::new(
            Token::of::<T>(),
            T::dependencies(),
            boxed(|container| {
                Box::pin(async move {
                    let value = T::build(container).await?;
                    Ok(store::<T>(Arc::new(value)))
                })
            }),
        )
    }

    /// Provides the interface `I` using the implementation `T`.
    ///
    /// `coerce` performs the unsizing step; the [`bind!`](crate::bind) macro writes
    /// it for you: `bind!(dyn UserRepository => PgUserRepository)`.
    pub fn bind<I, T>(coerce: fn(Arc<T>) -> Arc<I>) -> Self
    where
        I: ?Sized + Send + Sync + 'static,
        T: Injectable,
    {
        Self::new(
            Token::of::<I>(),
            T::dependencies(),
            boxed(move |container| {
                Box::pin(async move {
                    let concrete = Arc::new(T::build(container).await?);
                    Ok(store::<I>(coerce(concrete)))
                })
            }),
        )
    }

    /// Adds `T` to the set of implementations of `I`, which consumers receive as
    /// [`Multi<I>`](Multi). Prefer the [`contribute!`](crate::contribute) macro.
    ///
    /// Anyone, in any module, can contribute; nobody needs to export or import
    /// anything, because the set itself is visible everywhere (think plug-ins:
    /// health checks, event listeners, validators). The members are built like any
    /// provider and must be singletons. Lifecycle hooks do not apply to members:
    /// give a member's resources a provider of their own if they need them.
    ///
    /// A set exists as soon as one member does. To allow an empty set, also declare
    /// it with [`multi`](Self::multi).
    pub fn contribute<I, T>(coerce: fn(Arc<T>) -> Arc<I>) -> Self
    where
        I: ?Sized + Send + Sync + 'static,
        T: Injectable,
    {
        let mut provider = Self::new(
            Token::of::<Member<I, T>>(),
            T::dependencies(),
            boxed(move |container| {
                Box::pin(async move {
                    let concrete = Arc::new(T::build(container).await?);
                    Ok(store::<I>(coerce(concrete)))
                })
            }),
        );
        provider.multi = Some(MultiRole {
            target: Token::of::<Multi<I>>(),
            assemble: assemble::<I>,
            member: true,
        });
        provider
    }

    /// Declares the set `Multi<I>` even if nothing contributes to it, so that
    /// consumers still start (with an empty set). Declaring it more than once is fine.
    pub fn multi<I: ?Sized + Send + Sync + 'static>() -> Self {
        let mut provider = Self::new(
            Token::of::<Member<I, ()>>(),
            Vec::new(),
            boxed(|_| Box::pin(async { Ok(store::<()>(Arc::new(()))) })),
        );
        provider.multi = Some(MultiRole {
            target: Token::of::<Multi<I>>(),
            assemble: assemble::<I>,
            member: false,
        });
        provider
    }

    /// The set a graph assembles for `target` from `members`.
    pub(crate) fn collection(target: Token, assemble: Assemble, members: Vec<Token>) -> Self {
        let built: Arc<[Token]> = members.clone().into();
        Self::new(
            target,
            members,
            boxed(move |container| {
                let built = Arc::clone(&built);
                Box::pin(async move { assemble(container, &built) })
            }),
        )
    }

    pub(crate) fn multi_role(&self) -> Option<&MultiRole> {
        self.multi.as_ref()
    }

    /// Provides an already-constructed value (configuration, a connection pool
    /// created before bootstrap).
    pub fn value<T: Send + Sync + 'static>(value: T) -> Self {
        Self::instance::<T>(Arc::new(value))
    }

    /// Provides an already-shared instance under the token `T`, which may be a
    /// trait object.
    ///
    /// This is the natural way to inject a test double you keep a handle to:
    ///
    /// ```
    /// # use std::sync::Arc;
    /// # use nestrs_core::Provider;
    /// trait Mailer: Send + Sync {}
    /// #[derive(Default)]
    /// struct FakeMailer;
    /// impl Mailer for FakeMailer {}
    ///
    /// let fake = Arc::new(FakeMailer);
    /// let provider = Provider::instance::<dyn Mailer>(fake.clone()); // keep `fake` to assert on later
    /// # let _ = provider;
    /// ```
    pub fn instance<T>(instance: Arc<T>) -> Self
    where
        T: ?Sized + Send + Sync + 'static,
    {
        Self::new(
            Token::of::<T>(),
            Vec::new(),
            boxed(move |_| {
                let shared = Arc::clone(&instance);
                Box::pin(async move { Ok(store::<T>(shared)) })
            }),
        )
    }

    /// Provides `T` from an async closure, for anything that needs `.await` to
    /// set up (database pools, remote config).
    ///
    /// `dependencies` must list what the closure resolves from the container.
    ///
    /// ```
    /// # use nestrs_core::{Provider, Token, DiError};
    /// struct Pool;
    /// let provider = Provider::factory::<Pool>(vec![], |_container| {
    ///     Box::pin(async { Ok(Pool) })
    /// });
    /// # let _ = provider;
    /// ```
    pub fn factory<T>(
        dependencies: Vec<Token>,
        factory: impl for<'a> Fn(&'a Container) -> BoxFuture<'a, Result<T, DiError>> + Send + Sync + 'static,
    ) -> Self
    where
        T: Send + Sync + 'static,
    {
        Self::new(
            Token::of::<T>(),
            dependencies,
            boxed(move |container| {
                let pending = factory(container);
                Box::pin(async move { Ok(store::<T>(Arc::new(pending.await?))) })
            }),
        )
    }

    /// Like [`factory`](Self::factory), for a factory that returns a shared handle,
    /// which may be a trait object: choose an implementation at build time.
    ///
    /// ```
    /// # use std::sync::Arc;
    /// # use nestrs_core::{Provider, Token};
    /// trait Clock: Send + Sync {}
    /// struct System;
    /// impl Clock for System {}
    /// let provider = Provider::factory_arc::<dyn Clock>(vec![], |_| {
    ///     Box::pin(async { Ok(Arc::new(System) as Arc<dyn Clock>) })
    /// });
    /// # let _ = provider;
    /// ```
    pub fn factory_arc<T>(
        dependencies: Vec<Token>,
        factory: impl for<'a> Fn(&'a Container) -> BoxFuture<'a, Result<Arc<T>, DiError>> + Send + Sync + 'static,
    ) -> Self
    where
        T: ?Sized + Send + Sync + 'static,
    {
        Self::new(
            Token::of::<T>(),
            dependencies,
            boxed(move |container| {
                let pending = factory(container);
                Box::pin(async move { Ok(store::<T>(pending.await?)) })
            }),
        )
    }

    /// Declares additional dependencies that are not resolved during `build` but
    /// still have to exist and be visible, e.g. guards resolved when a controller
    /// is mounted.
    pub fn with_extra_dependencies(mut self, extra: impl IntoIterator<Item = Token>) -> Self {
        self.dependencies.extend(extra);
        self
    }

    /// Runs `T`'s [`OnModuleInit`] once the whole graph is built.
    ///
    /// `T` is normally the provider's own (concrete) type. For a `bind!`-ed
    /// interface use [`on_init`](Self::on_init) instead. Hooks belong to the
    /// provider: an override replaces them along with the provider.
    pub fn with_init<T: OnModuleInit>(self) -> Self {
        self.on_init::<T>(|instance| Box::pin(async move { instance.on_module_init().await }))
    }

    /// Runs `T`'s [`OnModuleDestroy`] at shutdown, before its dependencies are destroyed.
    pub fn with_destroy<T: OnModuleDestroy>(self) -> Self {
        self.on_destroy::<T>(|instance| Box::pin(async move { instance.on_module_destroy().await }))
    }

    /// Runs `hook` with the resolved `T` after the graph is built. `T` may be a
    /// trait object, so this also serves `bind!`-ed interfaces.
    pub fn on_init<T>(
        mut self,
        hook: impl Fn(Arc<T>) -> BoxFuture<'static, Result<(), DiError>> + Send + Sync + 'static,
    ) -> Self
    where
        T: ?Sized + Send + Sync + 'static,
    {
        self.init.push(resolving_hook(hook));
        self
    }

    /// Runs `hook` with the resolved `T` at shutdown. `T` may be a trait object.
    pub fn on_destroy<T>(
        mut self,
        hook: impl Fn(Arc<T>) -> BoxFuture<'static, Result<(), DiError>> + Send + Sync + 'static,
    ) -> Self
    where
        T: ?Sized + Send + Sync + 'static,
    {
        self.destroy.push(resolving_hook(hook));
        self
    }

    /// The type this provider supplies.
    pub fn token(&self) -> Token {
        self.token
    }

    /// The types that must be built before this one.
    pub fn dependencies(&self) -> &[Token] {
        &self.dependencies
    }

    pub(crate) fn init_hooks(&self) -> &[Hook] {
        &self.init
    }

    pub(crate) fn destroy_hooks(&self) -> &[Hook] {
        &self.destroy
    }

    pub(crate) fn build<'a>(&'a self, container: &'a Container) -> BoxFuture<'a, Result<Stored, DiError>> {
        (self.factory)(container)
    }
}

impl std::fmt::Debug for Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Provider")
            .field("token", &self.token)
            .field("dependencies", &self.dependencies)
            .finish()
    }
}

/// Adds an implementation to a multi-binding:
/// `contribute!(dyn HealthIndicator => DatabaseIndicator)`.
///
/// Consumers depend on `Arc<Multi<dyn HealthIndicator>>`; see [`Provider::contribute`].
#[macro_export]
macro_rules! contribute {
    (dyn $interface:path => $implementation:ty) => {
        $crate::Provider::contribute::<dyn $interface, $implementation>(
            |instance: ::std::sync::Arc<$implementation>| -> ::std::sync::Arc<dyn $interface> { instance },
        )
    };
}

/// Binds an interface to an implementation: `bind!(dyn UserRepository => PgUserRepository)`.
///
/// Expands to a [`Provider`] registered under the trait object's token, built by
/// the implementation's [`Injectable`] impl. Consumers depend on
/// `Arc<dyn UserRepository>` and never name the concrete type.
#[macro_export]
macro_rules! bind {
    (dyn $interface:path => $implementation:ty) => {
        $crate::Provider::bind::<dyn $interface, $implementation>(
            |instance: ::std::sync::Arc<$implementation>| -> ::std::sync::Arc<dyn $interface> { instance },
        )
    };
}
