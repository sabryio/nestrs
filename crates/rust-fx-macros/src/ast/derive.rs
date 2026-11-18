use proc_macro2::TokenStream;
use syn::DeriveInput;

/// Both derives read a plain `struct`/`enum` definition.
pub(crate) fn parse(args: TokenStream, item: TokenStream) -> syn::Result<DeriveInput> {
    debug_assert!(args.is_empty(), "derives have no arguments");
    syn::parse2(item)
}
