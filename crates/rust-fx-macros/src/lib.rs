//! Procedural macros for [`rust-fx`](https://docs.rs/rust-fx).
//!
//! Use them through the `fx` crate (`fx::component`, `fx::bind`, `fx::module`,
//! `fx::Params`, `fx::Component`); this crate is an implementation detail.
//!
//! # Architecture
//!
//! The macros share one compiler pipeline (see `pipeline.rs`): **parse** (`ast/`) →
//! **analyze** (`sema/`, reports every rule violation) → **IR** (`ir.rs`) → **generate**
//! (`codegen/`). `lib.rs` only declares the public macros; `macros.rs` says which stage
//! implementations each one uses.

use proc_macro::TokenStream;

mod ast;
mod codegen;
mod diagnostics;
mod ir;
mod macros;
mod pipeline;
mod sema;
mod support;

#[cfg(test)]
mod tests;

/// Turns an `impl` block into an `fx` component.
///
/// Inside the block, these attributes mark what the component contributes:
///
/// | Attribute | On | Effect |
/// |---|---|---|
/// | `#[fx::constructor]` | associated fn | a constructor; its parameters are its dependencies |
/// | /// | `#[fx::constructor(name = "x")]` | associated fn | register it under a name |
/// | `#[fx::constructor(group)]` / `group = "x"` | associated fn | contribute it to a group |
/// | `#[fx::on_start]` / `#[fx::on_stop]` | `&self` method, sync or `async` | lifecycle hooks of the built value |
/// | `#[fx::invoke]` | associated fn | run once at startup |
/// | `#[fx::decorate]` | associated fn | wrap a value |
///
/// The macro also implements `fx::Component` for the type, which is what allows the
/// constructors to return `Self` instead of `Arc<Self>`.
///
/// ```ignore
/// #[fx::component]
/// impl Clock {
///     #[fx::constructor]
///     pub fn new() -> Self { Self }
/// }
/// ```
///
/// Register the whole component with `providers = [Clock]` in a module (or
/// `App::component::<Clock>()`): everything declared in the block comes along. Listing
/// a single function (`providers = [Clock::new]`) registers just that function and
/// ignores the attribute options.
///
/// Limitations: the type must not be generic, and there must be one annotated `impl`
/// block per type.
#[proc_macro_attribute]
pub fn component(args: TokenStream, item: TokenStream) -> TokenStream {
    pipeline::run::<macros::ComponentMacro>(args, item)
}

/// Declares that a type may be used as an interface: `fx.As`, written where the trait is
/// implemented.
///
/// ```ignore
/// #[fx::bind]
/// impl Clock for CounterClock {
///     fn tick(&self) -> u32 { self.0.fetch_add(1, Ordering::SeqCst) }
/// }
/// ```
///
/// The attribute leaves the impl untouched and adds two things: an `fx::Bound<dyn Clock>`
/// impl (so the cast is checked by the compiler and never written by hand), and a
/// registration that is applied whenever `CounterClock` is registered as a component, by
/// `providers = [CounterClock]` or `Options::component::<CounterClock>()`. From then on
/// `Arc<dyn Clock>` can be injected, and a module can `exports = [dyn Clock]`, which
/// publishes the interface while the concrete type stays private.
///
/// Options choose where the interface value goes, like on a constructor:
/// `#[fx::bind(group)]`, `#[fx::bind(group = "jobs")]`, `#[fx::bind(name = "primary")]`.
///
/// Limitations: the impl must not be generic, and the trait needs `Send + Sync` as
/// supertraits. A binding is only applied when the type is registered as a *component*;
/// listing a single constructor function (`providers = [CounterClock::new]`) does not
/// register bindings.
#[proc_macro_attribute]
pub fn bind(args: TokenStream, item: TokenStream) -> TokenStream {
    pipeline::run::<macros::BindMacro>(args, item)
}

/// Declares an `fx` module on a unit struct.
///
/// ```ignore
/// #[fx::module(
///     name = "data",
///     imports = [ConfigModule],
///     providers = [Database, SqlRepo],
///     invokes = [migrate],
///     decorators = [traced],
///     exports = [Repo, Database as "primary"],
/// )]
/// pub struct DataModule;
/// ```
///
/// - `imports`: other modules; their exports become visible here.
/// - `providers`: a component type (`Database`) registers everything its `impl`
///   declares; a function path or any expression registers one constructor.
/// - `invokes`, `decorators`: functions.
/// - `exports`: what importers may use. Everything else stays private to the module.
/// - `name`: defaults to the struct's name.
#[proc_macro_attribute]
pub fn module(args: TokenStream, item: TokenStream) -> TokenStream {
    pipeline::run::<macros::ModuleMacro>(args, item)
}

/// Bundles several parameters into one struct, like fx's `fx.In`.
///
/// ```ignore
/// #[derive(fx::Params)]
/// struct Deps {
///     db: Arc<Database>,
///     jobs: fx::Group<dyn Job>,
///     cache: Option<Arc<Cache>>,
/// }
///
/// fn serve(deps: Deps) { /* ... */ }
/// ```
///
/// Every field type must itself be an `fx::Extract`. Generic structs are not supported.
#[proc_macro_derive(Params)]
pub fn params(item: TokenStream) -> TokenStream {
    pipeline::run::<macros::ParamsMacro>(TokenStream::new(), item)
}

/// Lets a type be returned by value from constructors (`-> Self`), without an
/// `#[fx::component]` block.
///
/// ```ignore
/// #[derive(fx::Component)]
/// struct Config { url: String }
///
/// fn config() -> Config { Config { url: "..".into() } }
/// ```
#[proc_macro_derive(Component)]
pub fn component_derive(item: TokenStream) -> TokenStream {
    pipeline::run::<macros::ComponentDerive>(TokenStream::new(), item)
}
