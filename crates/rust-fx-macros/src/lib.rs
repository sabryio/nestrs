//! Procedural macros for [`rust-fx`](https://docs.rs/rust-fx).
//!
//! Use them through the `fx` crate (`fx::component`, `fx::module`, `fx::Params`,
//! `fx::Component`); this crate is an implementation detail.

use proc_macro::TokenStream;

mod component;
mod derive;
mod module;
mod util;

/// Turns an `impl` block into an `fx` component.
///
/// Inside the block, these attributes mark what the component contributes:
///
/// | Attribute | On | Effect |
/// |---|---|---|
/// | `#[fx::provide]` | associated fn | a constructor; its parameters are its dependencies |
/// | `#[fx::provide(as = dyn Port)]` | associated fn | provide it as the interface (`fx.As`) |
/// | `#[fx::provide(name = "x")]` | associated fn | register it under a name |
/// | `#[fx::provide(group)]` / `group = "x"` | associated fn | contribute it to a group |
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
///     #[fx::provide]
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
    util::run(|| {
        let args = proc_macro2::TokenStream::from(args);
        if !args.is_empty() {
            return Err(syn::Error::new_spanned(args, "#[fx::component] takes no arguments"));
        }
        component::expand(syn::parse(item)?)
    })
}

/// Declares an `fx` module on a unit struct.
///
/// ```ignore
/// #[fx::module(
///     name = "data",
///     imports = [ConfigModule],
///     providers = [Database, fx::bind!(dyn Repo, SqlRepo::new)],
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
    util::run(|| module::expand(args.into(), syn::parse(item)?))
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
    util::run(|| derive::params(syn::parse(item)?))
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
    util::run(|| derive::component(syn::parse(item)?))
}
