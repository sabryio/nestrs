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
