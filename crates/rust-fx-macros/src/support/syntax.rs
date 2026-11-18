use syn::Path;

/// The last segment of `path` if it is `fx::<name>`, `::fx::<name>` or a bare `<name>`.
pub(crate) fn fx_name(path: &Path) -> Option<String> {
    let segments: Vec<_> = path.segments.iter().collect();
    match segments.as_slice() {
        [only] => Some(only.ident.to_string()),
        [.., fx, last] if fx.ident == "fx" => Some(last.ident.to_string()),
        _ => None,
    }
}

/// Whether a path names a type by convention (last segment starts uppercase).
pub(crate) fn looks_like_type(path: &Path) -> bool {
    path.segments
        .last()
        .and_then(|segment| segment.ident.to_string().chars().next())
        .is_some_and(char::is_uppercase)
}
