//! # fx
//!
//! The application framework behind `purlin`, usable on its own: dependency
//! injection, modules, lifecycle hooks and a graceful runtime, with no web stack
//! attached. It is modelled on Uber's [fx](https://uber-go.github.io/fx/) for Go.
//!
//! ```
//! use std::sync::Arc;
//!
//! struct Config { greeting: String }
//! struct Greeter { config: Arc<Config> }
//!
//! fn config() -> Arc<Config> { Arc::new(Config { greeting: "hello".into() }) }
//! fn greeter(config: Arc<Config>) -> Arc<Greeter> { Arc::new(Greeter { config }) }
//! fn main_job(greeter: Arc<Greeter>) { println!("{}", greeter.config.greeting); }
//!
//! # #[tokio::main(flavor = "current_thread")]
//! # async fn main() -> fx::Result<()> {
//! fx::App::new()
//!     .constructor(config)      // fx.Provide: a function is a constructor
//!     .constructor(greeter)     // its arguments are its dependencies
//!     .invoke(main_job)     // fx.Invoke: runs once, dependencies first
//!     .run_until_started()
//!     .await
//! # }
//! ```
//!
//! ## The pieces
//!
//! - **Constructors** are plain functions, sync or `async`, taking [`Arc<T>`](std::sync::Arc),
//!   [`Option<Arc<T>>`], [`Named`], [`Group`], [`Lifecycle`] or [`Shutdowner`] and
//!   returning a [`Component`], an `Arc<T>` or a `Result` of either. See [`extract`].
//! - **Invokes** run once at startup; only what they need is built.
//! - **Decorators** wrap a value for everything in their scope.
//! - **Modules** bundle all of that behind a private scope with explicit exports.
//! - **[`App`]** validates the graph, starts it, waits for a signal, and stops it in
//!   reverse order, with timeouts.
//!
//! Wiring mistakes (missing, hidden, duplicate and circular dependencies) are
//! reported by [`App::validate`] and again before anything runs, with the full
//! dependency chain and the source location of each provider.
//!
//! ## Macros
//!
//! With the default `macros` feature, `#[fx::component]` and `#[fx::module]` remove the
//! boilerplate; see their documentation.

#![forbid(unsafe_code)]

// Lets the macros' `::fx::` paths resolve inside this crate's own tests and docs.
extern crate self as fx;

mod app;
mod bind;
mod descriptor;
mod error;
pub mod extract;
mod func;
mod graph;
mod key;
mod lifecycle;
mod module;
mod observer;
mod options;
mod scope;
mod shutdown;
mod value;

#[cfg(feature = "test")]
pub mod test;

pub use app::App;
pub use bind::{Bound, bind};
pub use descriptor::{
    Decorate, Decorator, IntoDecorator, IntoInvoke, IntoProvider, Invoke, Lifetime, Provide, Provider, constructor,
    decorate, invoke,
};
pub use error::{BoxError, Error, Phase, Result};
pub use extract::{
    Args, Context, DepKind, DepSpec, Extract, Group, GroupName, Named, Optional, Resolved, Tag, Unnamed,
};
pub use func::{Component, Constructor, CtorFuture, Invocation, InvokeOutcome, Outcome};
pub use key::{Key, Origin};
pub use lifecycle::{Hook, HookOutcome, Lifecycle};
pub use module::{Export, Module, ModuleDef, ModuleRef};
pub use observer::{Event, Observer, Silent, Tracing};
pub use options::Options;
pub use scope::{RequestScope, Scopes};
pub use shutdown::{Shutdowner, Signal};

/// The async runtime `fx` is built on, re-exported so that an fx program does not need its
/// own `tokio` dependency to have a `main`, spawn tasks or sleep. `#[fx::main]` runs on it.
///
/// Features other than `rt`, `rt-multi-thread`, `sync`, `time`, `signal` and `macros` (say
/// `net` or `fs`) are enabled by depending on `tokio` yourself; Cargo unifies the features.
pub use tokio;
pub use value::BoxFuture;

#[cfg(feature = "macros")]
pub use rust_fx_macros::{Component, Params, bind, component, main, module, port};

/// Support for macro-generated code. Not part of the public API.
#[doc(hidden)]
pub mod __private {
    pub use crate::bind::{Binding, Placement};
    pub use inventory;
    pub use tokio;
}

/// Declares a unit struct that names values: `fx::tag!(Primary = "primary");`.
///
/// Use it with [`Named`] and with named groups.
#[macro_export]
macro_rules! tag {
    ($(#[$meta:meta])* $vis:vis $name:ident = $text:literal) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy)]
        $vis struct $name;

        impl $crate::Tag for $name {
            const NAME: &'static str = $text;
        }
    };
}

/// The names most programs need.
pub mod prelude {
    pub use crate::{
        App, Bound, Component, Error, Group, Lifecycle, Module, Named, Optional, Options, Result, Shutdowner, bind,
        constructor, decorate, invoke,
    };
    #[cfg(feature = "macros")]
    pub use rust_fx_macros::{Params, component, module, port};
}
