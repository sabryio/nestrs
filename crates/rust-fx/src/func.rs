//! Turning plain functions into constructors, invokes and decorators.
//!
//! Every function whose parameters implement [`Extract`] and whose return type is an
//! [`Outcome`] (or, for invokes, an [`InvokeOutcome`]) is accepted, sync or `async`.
//! The impls are generated for up to twelve parameters, the same trick `axum` uses
//! for handlers.

use std::future::Future;
use std::sync::Arc;

use crate::Options;
use crate::error::{BoxError, Result};
use crate::extract::{Args, Context, DepSpec, Extract};
use crate::value::BoxFuture;

/// Marker for synchronous functions.
#[doc(hidden)]
pub struct SyncFn;
/// Marker for `async` functions.
#[doc(hidden)]
pub struct AsyncFn;

/// A type that opts in to being returned *by value* from a constructor.
///
/// Returning `Self` (or a `Result` of it) is shorthand for returning `Arc<Self>`: the
/// container wraps the value in an `Arc` once. Derive it with
/// `#[derive(fx::Component)]`, or get it from `#[fx::component]` on an `impl` block.
/// For types you do not own, return `Arc<T>` instead.
///
/// `register` lets a component bring its own providers, invokes and decorators; the
/// default registers nothing.
pub trait Component: Send + Sync + Sized + 'static {
    /// Adds everything this component declares to `options`.
    fn register(options: Options) -> Options {
        options
    }
}

/// What a constructor may return: a [`Component`], an `Arc<T>`, or a `Result` of either.
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be returned from a constructor",
    label = "constructors return `Arc<T>` or a `Component`",
    note = "return `Arc<{Self}>`, or add `#[derive(fx::Component)]` to the type to return it by value"
)]
pub trait Outcome: Send + 'static {
    /// The type that ends up in the graph (possibly `dyn Trait`).
    type Value: ?Sized + Send + Sync + 'static;

    /// Converts the output into the shared handle the graph stores.
    ///
    /// # Errors
    ///
    /// Whatever the constructor failed with.
    fn into_shared(self) -> std::result::Result<Arc<Self::Value>, BoxError>;
}

impl<C: Component> Outcome for C {
    type Value = C;

    fn into_shared(self) -> std::result::Result<Arc<C>, BoxError> {
        Ok(Arc::new(self))
    }
}

impl<T: ?Sized + Send + Sync + 'static> Outcome for Arc<T> {
    type Value = T;

    fn into_shared(self) -> std::result::Result<Arc<T>, BoxError> {
        Ok(self)
    }
}

impl<R: Outcome, E: Into<BoxError> + Send + 'static> Outcome for std::result::Result<R, E> {
    type Value = R::Value;

    fn into_shared(self) -> std::result::Result<Arc<R::Value>, BoxError> {
        self.map_err(Into::into).and_then(Outcome::into_shared)
    }
}

/// What an invoked function may return: nothing, or a `Result<(), E>`.
pub trait InvokeOutcome: Send + 'static {
    /// Converts the output into a uniform result.
    ///
    /// # Errors
    ///
    /// Whatever the function failed with.
    fn into_result(self) -> std::result::Result<(), BoxError>;
}

impl InvokeOutcome for () {
    fn into_result(self) -> std::result::Result<(), BoxError> {
        Ok(())
    }
}

impl<E: Into<BoxError> + Send + 'static> InvokeOutcome for std::result::Result<(), E> {
    fn into_result(self) -> std::result::Result<(), BoxError> {
        self.map_err(Into::into)
    }
}

/// The future a constructor call produces.
pub type CtorFuture<V> = BoxFuture<'static, std::result::Result<Arc<V>, BoxError>>;

/// A function the graph can call to build a value.
///
/// Implemented for every `Fn(A, B, ..) -> R` (or `-> impl Future<Output = R>`) whose
/// parameters are [`Extract`] and whose `R` is an [`Outcome`]. `Marker` only exists to
/// keep the sync and async impls apart.
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be used as a constructor",
    label = "not a valid constructor",
    note = "a constructor is a function whose parameters are `Arc<T>`, `Option<Arc<T>>`, `fx::Named`, `fx::Group`, `fx::Lifecycle` or `fx::Shutdowner` (or a `#[derive(fx::Params)]` struct)",
    note = "and whose return type is an `Arc<T>`, a type with `#[derive(fx::Component)]` / `#[fx::component]`, or a `Result` of either"
)]
pub trait Constructor<Marker>: Send + Sync + 'static {
    /// What the function provides.
    type Value: ?Sized + Send + Sync + 'static;

    /// What the function needs.
    fn specs() -> Vec<DepSpec>;

    /// Extracts the arguments and starts the call.
    ///
    /// # Errors
    ///
    /// [`Error::Internal`](crate::Error::Internal) when the arguments do not match.
    fn call(&self, args: &mut Args, ctx: &Context) -> Result<CtorFuture<Self::Value>>;
}

/// A function the graph can call once at startup.
///
/// The same shape as [`Constructor`], for functions that return nothing (or a
/// `Result<(), E>`).
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be used as an invoke",
    label = "not a valid invoke function",
    note = "an invoke is a function whose parameters are `Arc<T>`, `Option<Arc<T>>`, `fx::Named`, `fx::Group`, `fx::Lifecycle` or `fx::Shutdowner`",
    note = "and which returns `()` or a `Result<(), E>`"
)]
pub trait Invocation<Marker>: Send + Sync + 'static {
    /// What the function needs.
    fn specs() -> Vec<DepSpec>;

    /// Extracts the arguments and starts the call.
    ///
    /// # Errors
    ///
    /// [`Error::Internal`](crate::Error::Internal) when the arguments do not match.
    fn call(&self, args: &mut Args, ctx: &Context) -> Result<BoxFuture<'static, std::result::Result<(), BoxError>>>;
}

macro_rules! impl_functions {
    ($($T:ident),*) => {
        #[allow(non_snake_case, unused_variables, unused_mut)]
        impl<F, R, $($T),*> Constructor<(SyncFn, ($($T,)*))> for F
        where
            F: Fn($($T),*) -> R + Send + Sync + 'static,
            R: Outcome,
            $($T: Extract),*
        {
            type Value = R::Value;

            fn specs() -> Vec<DepSpec> {
                let mut out = Vec::new();
                $($T::specs(&mut out);)*
                out
            }

            fn call(&self, args: &mut Args, ctx: &Context) -> Result<CtorFuture<R::Value>> {
                $(let $T = $T::extract(args, ctx)?;)*
                let output = (self)($($T),*);
                Ok(Box::pin(std::future::ready(output.into_shared())))
            }
        }

        #[allow(non_snake_case, unused_variables, unused_mut)]
        impl<F, Fut, R, $($T),*> Constructor<(AsyncFn, ($($T,)*))> for F
        where
            F: Fn($($T),*) -> Fut + Send + Sync + 'static,
            Fut: Future<Output = R> + Send + 'static,
            R: Outcome,
            $($T: Extract),*
        {
            type Value = R::Value;

            fn specs() -> Vec<DepSpec> {
                let mut out = Vec::new();
                $($T::specs(&mut out);)*
                out
            }

            fn call(&self, args: &mut Args, ctx: &Context) -> Result<CtorFuture<R::Value>> {
                $(let $T = $T::extract(args, ctx)?;)*
                let future = (self)($($T),*);
                Ok(Box::pin(async move { future.await.into_shared() }))
            }
        }

        #[allow(non_snake_case, unused_variables, unused_mut)]
        impl<F, R, $($T),*> Invocation<(SyncFn, ($($T,)*))> for F
        where
            F: Fn($($T),*) -> R + Send + Sync + 'static,
            R: InvokeOutcome,
            $($T: Extract),*
        {
            fn specs() -> Vec<DepSpec> {
                let mut out = Vec::new();
                $($T::specs(&mut out);)*
                out
            }

            fn call(&self, args: &mut Args, ctx: &Context) -> Result<BoxFuture<'static, std::result::Result<(), BoxError>>> {
                $(let $T = $T::extract(args, ctx)?;)*
                let output = (self)($($T),*);
                Ok(Box::pin(std::future::ready(output.into_result())))
            }
        }

        #[allow(non_snake_case, unused_variables, unused_mut)]
        impl<F, Fut, R, $($T),*> Invocation<(AsyncFn, ($($T,)*))> for F
        where
            F: Fn($($T),*) -> Fut + Send + Sync + 'static,
            Fut: Future<Output = R> + Send + 'static,
            R: InvokeOutcome,
            $($T: Extract),*
        {
            fn specs() -> Vec<DepSpec> {
                let mut out = Vec::new();
                $($T::specs(&mut out);)*
                out
            }

            fn call(&self, args: &mut Args, ctx: &Context) -> Result<BoxFuture<'static, std::result::Result<(), BoxError>>> {
                $(let $T = $T::extract(args, ctx)?;)*
                let future = (self)($($T),*);
                Ok(Box::pin(async move { future.await.into_result() }))
            }
        }
    };
}

impl_functions!();
impl_functions!(A1);
impl_functions!(A1, A2);
impl_functions!(A1, A2, A3);
impl_functions!(A1, A2, A3, A4);
impl_functions!(A1, A2, A3, A4, A5);
impl_functions!(A1, A2, A3, A4, A5, A6);
impl_functions!(A1, A2, A3, A4, A5, A6, A7);
impl_functions!(A1, A2, A3, A4, A5, A6, A7, A8);
impl_functions!(A1, A2, A3, A4, A5, A6, A7, A8, A9);
impl_functions!(A1, A2, A3, A4, A5, A6, A7, A8, A9, A10);
impl_functions!(A1, A2, A3, A4, A5, A6, A7, A8, A9, A10, A11);
impl_functions!(A1, A2, A3, A4, A5, A6, A7, A8, A9, A10, A11, A12);
