use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use syn::{Attribute, Path};

/// Runs an expansion and turns its error into `compile_error!`.
pub(crate) fn run(expand: impl FnOnce() -> syn::Result<TokenStream2>) -> TokenStream {
    match expand() {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

/// The last segment of `path` if it is `fx::<name>`, `::fx::<name>` or a bare `<name>`.
pub(crate) fn fx_name(path: &Path) -> Option<String> {
    let segments: Vec<_> = path.segments.iter().collect();
    match segments.as_slice() {
        [only] => Some(only.ident.to_string()),
        [.., fx, last] if fx.ident == "fx" => Some(last.ident.to_string()),
        _ => None,
    }
}

/// If `attr` is one of the `fx` member attributes, returns which one.
pub(crate) fn member_attr(attr: &Attribute) -> Option<&'static str> {
    const NAMES: [&str; 5] = ["provide", "on_start", "on_stop", "invoke", "decorate"];
    let name = fx_name(attr.path())?;
    NAMES.into_iter().find(|candidate| *candidate == name)
}

/// Whether a path names a type by convention (last segment starts uppercase).
pub(crate) fn looks_like_type(path: &Path) -> bool {
    path.segments
        .last()
        .and_then(|segment| segment.ident.to_string().chars().next())
        .is_some_and(char::is_uppercase)
}

/// The path under which the expanded code reaches the `fx` crate.
///
/// `::fx` when the caller depends on `rust-fx` directly, `::purlin::fx` when it only
/// depends on `purlin` (which re-exports it), or whatever name the caller gave the
/// dependency in its manifest.
pub(crate) fn fx_path() -> TokenStream2 {
    use proc_macro_crate::{FoundCrate, crate_name};
    use quote::quote;

    let named = |name: &str| proc_macro2::Ident::new(&name.replace('-', "_"), proc_macro2::Span::call_site());
    match crate_name("rust-fx") {
        // The library of the `rust-fx` package is called `fx`, unless it was renamed.
        Ok(FoundCrate::Itself) => quote!(::fx),
        Ok(FoundCrate::Name(name)) if name == "rust-fx" || name == "rust_fx" => quote!(::fx),
        Ok(FoundCrate::Name(name)) => {
            let name = named(&name);
            quote!(::#name)
        }
        Err(_) => match crate_name("purlin") {
            Ok(FoundCrate::Name(name)) => {
                let name = named(&name);
                quote!(::#name::fx)
            }
            Ok(FoundCrate::Itself) | Err(_) => quote!(::fx),
        },
    }
}
