use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;

/// The path under which the expanded code reaches the `fx` crate.
///
/// `::fx` when the caller depends on `rust-fx` (the library is called `fx`), or whatever name
/// the caller gave the dependency in its manifest. No other crate is consulted: `fx` does not
/// know `purlin`, nor any other framework built on it.
pub(crate) fn fx_path() -> TokenStream {
    use proc_macro_crate::{FoundCrate, crate_name};

    let named = |name: &str| Ident::new(&name.replace('-', "_"), Span::call_site());
    match crate_name("rust-fx") {
        // The library of the `rust-fx` package is called `fx`, unless it was renamed.
        Ok(FoundCrate::Itself) => quote!(::fx),
        Ok(FoundCrate::Name(name)) if name == "rust-fx" || name == "rust_fx" => quote!(::fx),
        Ok(FoundCrate::Name(name)) => {
            let name = named(&name);
            quote!(::#name)
        }
        // Not found in the manifest (macro unit tests, doctests): the conventional name.
        Err(_) => quote!(::fx),
    }
}
