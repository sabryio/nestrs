//! Edits to existing Rust source, found by parsing it rather than by matching text.
//!
//! The file is parsed with `syn` only to *locate* things (spans give exact byte offsets); the
//! change is then spliced into the original text, so formatting, comments and everything else are
//! left exactly as written.

use proc_macro2::{Delimiter, TokenTree};
use syn::spanned::Spanned;
use syn::{Item, Meta};

/// Why a file could not be patched. Always safe to show: the file is untouched.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PatchError {
    /// The file is not valid Rust.
    #[error("cannot parse the file: {0}")]
    Parse(String),
    /// There is no `#[module(..)]` struct in it.
    #[error("no #[module(..)] struct found")]
    NoModule,
}

/// Where a 1-based line and 0-based char column fall in `source`, as a byte offset.
fn offset(source: &str, line: usize, column: usize) -> usize {
    let mut start = 0;
    for (index, text) in source.split_inclusive('\n').enumerate() {
        if index + 1 == line {
            let in_line = text.char_indices().nth(column).map_or(text.len(), |(byte, _)| byte);
            return start + in_line;
        }
        start += text.len();
    }
    source.len()
}

fn offset_of(source: &str, point: proc_macro2::LineColumn) -> usize {
    offset(source, point.line, point.column)
}

/// Makes `item` part of `key = [..]` (`imports`, `providers`, `controllers`..) of the file's
/// `#[module(..)]`, creating the entry if the attribute has none. `marker` is the identifier that
/// tells whether it is already there (`BillingModule` for `BillingModule`, `JobsModule` for
/// `JobsModule::in_memory()`). Returns `None` when it is.
pub fn add_module_entry(source: &str, key: &str, item: &str, marker: &str) -> Result<Option<String>, PatchError> {
    let file = syn::parse_file(source).map_err(|error| PatchError::Parse(error.to_string()))?;
    for found in &file.items {
        let Item::Struct(found) = found else { continue };
        let Some(attribute) = found.attrs.iter().find(|a| a.path().is_ident("module")) else {
            continue;
        };

        // `#[module]` with no arguments.
        let Meta::List(list) = &attribute.meta else {
            let end = offset_of(source, attribute.meta.span().end());
            let mut patched = source.to_owned();
            patched.replace_range(end..end, &format!("({key} = [{item}])"));
            return Ok(Some(patched));
        };

        let tokens: Vec<TokenTree> = list.tokens.clone().into_iter().collect();
        for index in 0..tokens.len() {
            if let (TokenTree::Ident(name), Some(TokenTree::Punct(eq)), Some(TokenTree::Group(group))) =
                (&tokens[index], tokens.get(index + 1), tokens.get(index + 2))
                && name == key
                && eq.as_char() == '='
                && group.delimiter() == Delimiter::Bracket
            {
                if contains_ident(group.stream(), marker) {
                    return Ok(None);
                }
                let open = offset_of(source, group.span_open().end());
                let empty = group.stream().is_empty();
                let mut patched = source.to_owned();
                patched.replace_range(open..open, &format!("{item}{}", if empty { "" } else { ", " }));
                return Ok(Some(patched));
            }
        }

        // No such entry yet: add it first.
        let open = offset_of(source, list.delimiter.span().open().end());
        let empty = list.tokens.is_empty();
        let mut patched = source.to_owned();
        patched.replace_range(
            open..open,
            &format!("{key} = [{item}]{}", if empty { "" } else { ", " }),
        );
        return Ok(Some(patched));
    }
    Err(PatchError::NoModule)
}

/// Whether `marker` appears as an identifier anywhere in `stream`, nested groups included.
fn contains_ident(stream: proc_macro2::TokenStream, marker: &str) -> bool {
    stream.into_iter().any(|token| match token {
        TokenTree::Ident(name) => name == marker,
        TokenTree::Group(group) => contains_ident(group.stream(), marker),
        _ => false,
    })
}

/// [`add_module_entry`] for `imports`, where the item is its own marker.
pub fn add_module_import(source: &str, module: &str) -> Result<Option<String>, PatchError> {
    add_module_entry(source, "imports", module, module)
}

/// Adds `use <path>;` after the file's last top-level `use`, or before the first item when there is
/// none. Returns `None` when an identical `use` is already there.
pub fn add_use(source: &str, path: &str) -> Result<Option<String>, PatchError> {
    let wanted = format!("use {path};");
    let file = syn::parse_file(source).map_err(|error| PatchError::Parse(error.to_string()))?;
    let uses: Vec<&syn::ItemUse> = file
        .items
        .iter()
        .filter_map(|item| if let Item::Use(u) = item { Some(u) } else { None })
        .collect();
    let normalised = |text: &str| text.split_whitespace().collect::<String>();
    for existing in &uses {
        let span = existing.span();
        let text = &source[offset_of(source, span.start())..offset_of(source, span.end())];
        if normalised(text) == normalised(&wanted) {
            return Ok(None);
        }
    }
    let mut patched = source.to_owned();
    if let Some(last) = uses.last() {
        let end = offset_of(source, last.span().end());
        patched.insert_str(end, &format!("\n{wanted}"));
    } else if let Some(first) = file.items.first() {
        let start = offset_of(source, first.span().start());
        patched.insert_str(start, &format!("{wanted}\n\n"));
    } else {
        patched.push_str(&format!("{wanted}\n"));
    }
    Ok(Some(patched))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_to_an_existing_imports_list() {
        let source = "#[module(imports = [Kit], providers = [A])]\npub struct AppModule;\n";
        let patched = add_module_import(source, "Billing").unwrap().unwrap();
        assert_eq!(
            patched,
            "#[module(imports = [Billing, Kit], providers = [A])]\npub struct AppModule;\n"
        );
    }

    #[test]
    fn survives_multiline_lists_comments_and_trailing_commas() {
        let source = "#[module(\n    imports = [\n        // infra\n        Persistence,\n        Kit,\n    ],\n)]\nstruct AppModule;\n";
        let patched = add_module_import(source, "Billing").unwrap().unwrap();
        assert!(patched.contains("imports = [Billing, \n        // infra"), "{patched}");
        assert!(syn::parse_file(&patched).is_ok(), "still valid Rust:\n{patched}");
    }

    #[test]
    fn creates_the_imports_entry_when_missing() {
        let patched = add_module_import("#[module(providers = [A])]\nstruct M;\n", "B")
            .unwrap()
            .unwrap();
        assert_eq!(patched, "#[module(imports = [B], providers = [A])]\nstruct M;\n");
        let bare = add_module_import("#[module]\nstruct M;\n", "B").unwrap().unwrap();
        assert_eq!(bare, "#[module(imports = [B])]\nstruct M;\n");
        let empty = add_module_import("#[module()]\nstruct M;\n", "B").unwrap().unwrap();
        assert_eq!(empty, "#[module(imports = [B])]\nstruct M;\n");
    }

    #[test]
    fn is_idempotent_and_ignores_lookalikes_elsewhere() {
        let source = "#[module(imports = [Billing], providers = [Kit])]\nstruct M;\n";
        assert_eq!(add_module_import(source, "Billing").unwrap(), None);
        // `Kit` is a provider here, not an import.
        assert!(add_module_import(source, "Kit").unwrap().is_some());
    }

    #[test]
    fn multibyte_text_before_the_attribute_does_not_shift_the_edit() {
        let source = "// café ☕\n#[module(imports = [Kit])]\nstruct M;\n";
        let patched = add_module_import(source, "B").unwrap().unwrap();
        assert_eq!(patched, "// café ☕\n#[module(imports = [B, Kit])]\nstruct M;\n");
    }

    #[test]
    fn an_expression_is_recognised_by_its_marker() {
        let source = "#[module(imports = [JobsModule::in_memory()], providers = [A])]\nstruct M;\n";
        assert_eq!(
            add_module_entry(source, "imports", "JobsModule::in_memory()", "JobsModule").unwrap(),
            None
        );
        let patched = add_module_entry(source, "providers", "PurgeJob", "PurgeJob")
            .unwrap()
            .unwrap();
        assert_eq!(
            patched,
            "#[module(imports = [JobsModule::in_memory()], providers = [PurgeJob, A])]\nstruct M;\n"
        );
    }

    #[test]
    fn reports_files_it_cannot_patch() {
        assert_eq!(add_module_import("struct Plain;", "B"), Err(PatchError::NoModule));
        assert!(matches!(add_module_import("fn (", "B"), Err(PatchError::Parse(_))));
    }

    #[test]
    fn adds_a_use_after_the_last_one_once() {
        let source = "use a::B;\nuse c::D;\n\n#[module]\nstruct M;\n";
        let patched = add_use(source, "super::X").unwrap().unwrap();
        assert_eq!(patched, "use a::B;\nuse c::D;\nuse super::X;\n\n#[module]\nstruct M;\n");
        assert_eq!(add_use(&patched, "super::X").unwrap(), None);
        let none = add_use("struct M;\n", "super::X").unwrap().unwrap();
        assert_eq!(none, "use super::X;\n\nstruct M;\n");
    }
}
