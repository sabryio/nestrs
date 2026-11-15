//! Attribute macros for nestrs.
//!
//! The macros are intentionally thin: they read declarations and emit plain trait
//! implementations from `nestrs-core` / `nestrs-http`. All behaviour lives in
//! ordinary generic code, which keeps compile times down and errors readable.
//!
//! Generated code refers to the facade crate as `::nestrs`, so applications depend
//! on `nestrs`, not on this crate directly.

mod controller;
mod injectable;
mod lifecycle;
mod module;
mod port;
mod provider;
mod util;

use proc_macro::TokenStream;
use syn::{ItemImpl, ItemStruct, parse_macro_input};

/// Derives [`Injectable`](../nestrs_core/trait.Injectable.html) for a struct.
///
/// Every field must be an injectable handle (`Arc<T>` or a port alias such as
/// `UserRepositoryPort`, resolved from the container) or carry `#[inject(default)]`
/// (filled with `Default::default()`).
///
/// - `#[injectable(provides = UserRepositoryPort)]` registers the type as the
///   implementation of that port instead of under its own type.
/// - If the type implements `OnModuleInit` / `OnModuleDestroy` (see `#[lifecycle]`),
///   the hooks are attached automatically.
///
/// ```ignore
/// #[injectable]
/// pub struct UsersService {
///     repo: UserRepositoryPort,
///     #[inject(default)]
///     cache: RwLock<HashMap<Uuid, User>>,
/// }
/// ```
#[proc_macro_attribute]
pub fn injectable(args: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(args as injectable::Args);
    let item = parse_macro_input!(item as ItemStruct);
    injectable::expand(args, item)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Turns an inherent `impl` block into an HTTP controller.
///
/// The argument is the route prefix. Methods opt in with `#[get]`, `#[post]`,
/// `#[put]`, `#[patch]` or `#[delete]` (optionally with a sub-path) and may carry
/// `#[guard(Type)]`; a `#[guard]` on the `impl` itself applies to every route. A
/// handler is an `async fn` taking `&self` followed by ordinary axum extractors.
///
/// ```ignore
/// #[controller("/users")]
/// impl UsersController {
///     #[get("/{id}")]
///     async fn find(&self, Path(id): Path<Uuid>) -> Result<Json<User>, HttpError> { .. }
///
///     #[post]
///     #[guard(AuthGuard)]
///     async fn create(&self, ValidatedJson(dto): ValidatedJson<NewUser>) -> Result<..> { .. }
/// }
/// ```
///
/// - `#[status(201)]` sets the default success status (a handler answering `200 OK`
///   answers `201 Created` instead) and documents it. `#[status(204)]` on a handler
///   returning `Result<(), _>` gives a `204 No Content`.
/// - `#[response(404, "Post not found")]` documents an extra error response in the
///   OpenAPI document.
///
/// Authorization is declarative: `#[roles("admin", "editor")]` (any of) and
/// `#[permissions("posts:delete")]` (all of), on the `impl` or a route, are checked
/// against the request's `Principal` after the `#[guard]`s have authenticated it:
/// 401 without a principal, 403 with too few privileges.
///
/// The struct itself must be `#[injectable]`.
#[proc_macro_attribute]
pub fn controller(args: TokenStream, item: TokenStream) -> TokenStream {
    let item = parse_macro_input!(item as ItemImpl);
    controller::expand(args.into(), item)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Describes a module on a unit struct.
///
/// ```ignore
/// #[module(
///     imports     = [AuthModule],
///     providers   = [UsersService, bind!(dyn UserRepository => InMemoryUserRepository)],
///     controllers = [UsersController],
///     exports     = [UsersService],
/// )]
/// pub struct UsersModule;
/// ```
///
/// `providers` entries are either a type implementing `Injectable` or any
/// expression evaluating to a `Provider` (`bind!(..)`, `Provider::value(..)`,
/// `Provider::factory(..)`).
#[proc_macro_attribute]
pub fn module(args: TokenStream, item: TokenStream) -> TokenStream {
    let item = parse_macro_input!(item as ItemStruct);
    module::expand(args.into(), item)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Declares a port: a trait that use cases depend on and adapters implement.
///
/// Applies `async_trait`, requires `Send + Sync + 'static`, and generates the alias
/// `<Trait>Port = Arc<dyn Trait>` (or the name you pass: `#[port(Users)]`), so the
/// rest of the code never writes `dyn`.
///
/// ```ignore
/// #[port]
/// pub trait UserRepository {
///     async fn find(&self, id: UserId) -> Result<Option<User>, RepositoryError>;
/// }
/// // generated: pub type UserRepositoryPort = Arc<dyn UserRepository>;
/// ```
#[proc_macro_attribute]
pub fn port(args: TokenStream, item: TokenStream) -> TokenStream {
    let item = parse_macro_input!(item as syn::ItemTrait);
    port::expand(args.into(), item)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Turns a function into a provider: parameters are its dependencies, the `Ok`
/// type of its return value is what it provides.
///
/// ```ignore
/// #[provider]
/// async fn user_repository(config: Arc<AppConfig>, db: Arc<Database>)
///     -> Result<UserRepositoryPort, DiError>
/// {
///     Ok(match config.storage { .. })
/// }
///
/// #[module(providers = [user_repository])]
/// ```
///
/// `#[provider(scope = request)]` (or `transient`) builds the value per request
/// instead of once.
///
/// The function may be sync or async and may return a plain value, an `Arc`, a port
/// alias, or a `Result` of any of those (the error only needs `Display`).
#[proc_macro_attribute]
pub fn provider(args: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(args as provider::Args);
    let item = parse_macro_input!(item as syn::ItemFn);
    provider::expand(args, item)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Marks lifecycle methods in an inherent `impl` block.
///
/// `#[on_init]` runs once the whole graph is built, `#[on_destroy]` at shutdown
/// (dependencies are destroyed after their dependents). Each method is
/// `async fn(&self) -> Result<(), E>` with `E: Display`.
///
/// ```ignore
/// #[lifecycle]
/// impl Database {
///     #[on_init]    async fn connect(&self) -> Result<(), sea_orm::DbErr> { .. }
///     #[on_destroy] async fn close(&self)   -> Result<(), sea_orm::DbErr> { .. }
/// }
/// ```
#[proc_macro_attribute]
pub fn lifecycle(args: TokenStream, item: TokenStream) -> TokenStream {
    if !args.is_empty() {
        return syn::Error::new(proc_macro2::Span::call_site(), "#[lifecycle] takes no arguments")
            .to_compile_error()
            .into();
    }
    let item = parse_macro_input!(item as ItemImpl);
    lifecycle::expand(item)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}
