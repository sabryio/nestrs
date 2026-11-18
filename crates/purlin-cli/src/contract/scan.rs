//! Reading the project's Rust files. Parsing only: no meaning is decided here.

use std::collections::BTreeMap;
use std::path::PathBuf;

use proc_macro2::{TokenStream, TokenTree};
use syn::{Attribute, Item, ItemEnum, ItemImpl, ItemStruct};

use super::ContractError;

/// A struct or enum found in the sources.
#[derive(Debug, Clone)]
pub enum TypeDef {
    Struct(ItemStruct),
    Enum(ItemEnum),
}

/// Everything the lowering step needs from the sources.
#[derive(Debug, Default)]
pub struct Sources {
    /// By name. A name defined twice is in `ambiguous` instead.
    pub types: BTreeMap<String, TypeDef>,
    pub ambiguous: Vec<String>,
    pub controllers: Vec<ItemImpl>,
    /// The argument of a `.prefix("..")` call, which is how `Service` mounts the API.
    pub prefix: Option<String>,
}

/// Parses every file. A file that is not valid Rust stops the run: a contract built from half
/// of the sources would be wrong in ways nobody could see.
pub fn scan(files: &[(PathBuf, String)]) -> Result<Sources, ContractError> {
    let mut sources = Sources::default();
    for (path, text) in files {
        let file = syn::parse_file(text).map_err(|error| ContractError::Parse {
            path: path.display().to_string(),
            message: error.to_string(),
        })?;
        collect(&file.items, &mut sources);
        if sources.prefix.is_none() && path.file_name().is_some_and(|name| name == "main.rs") {
            sources.prefix = mount_prefix(text);
        }
    }
    Ok(sources)
}

fn collect(items: &[Item], sources: &mut Sources) {
    for item in items {
        match item {
            Item::Struct(item) => register(sources, item.ident.to_string(), TypeDef::Struct(item.clone())),
            Item::Enum(item) => register(sources, item.ident.to_string(), TypeDef::Enum(item.clone())),
            Item::Impl(item) if item.trait_.is_none() && has_attr(&item.attrs, "controller") => {
                sources.controllers.push(item.clone());
            }
            Item::Mod(item) => {
                if let Some((_, inner)) = &item.content {
                    collect(inner, sources);
                }
            }
            _ => {}
        }
    }
}

fn register(sources: &mut Sources, name: String, def: TypeDef) {
    if sources.types.contains_key(&name) {
        sources.types.remove(&name);
        sources.ambiguous.push(name);
    } else if !sources.ambiguous.contains(&name) {
        sources.types.insert(name, def);
    }
}

pub fn has_attr(attrs: &[Attribute], name: &str) -> bool {
    attrs.iter().any(|attr| attr.path().is_ident(name))
}

/// `.prefix("/api")` in `source`, found in the token stream rather than by text matching.
fn mount_prefix(source: &str) -> Option<String> {
    fn walk(tokens: TokenStream) -> Option<String> {
        let mut previous_was_prefix = false;
        for token in tokens {
            match token {
                TokenTree::Ident(ident) => previous_was_prefix = ident == "prefix",
                TokenTree::Group(group) => {
                    if previous_was_prefix
                        && let Some(TokenTree::Literal(literal)) = group.stream().into_iter().next()
                        && let Ok(text) = syn::parse_str::<syn::LitStr>(&literal.to_string())
                    {
                        return Some(text.value());
                    }
                    previous_was_prefix = false;
                    if let Some(found) = walk(group.stream()) {
                        return Some(found);
                    }
                }
                _ => previous_was_prefix = false,
            }
        }
        None
    }
    walk(source.parse().ok()?)
}

/// The `///` lines of an item, without the leading space.
pub fn doc_lines(attrs: &[Attribute]) -> Vec<String> {
    attrs
        .iter()
        .filter(|attr| attr.path().is_ident("doc"))
        .filter_map(|attr| match &attr.meta {
            syn::Meta::NameValue(pair) => match &pair.value {
                syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(text),
                    ..
                }) => Some(text.value()),
                _ => None,
            },
            _ => None,
        })
        .map(|line| line.strip_prefix(' ').unwrap_or(&line).trim_end().to_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(text: &str) -> Vec<(PathBuf, String)> {
        vec![(PathBuf::from("src/lib.rs"), text.to_owned())]
    }

    #[test]
    fn finds_types_controllers_and_nested_modules() {
        let sources = scan(&files(
            "struct A; mod m { enum B { X } #[controller(\"/x\")] impl C { } } impl D {}",
        ))
        .unwrap();
        assert!(sources.types.contains_key("A") && sources.types.contains_key("B"));
        assert_eq!(sources.controllers.len(), 1, "only the #[controller] impl");
    }

    #[test]
    fn a_name_defined_twice_is_ambiguous_not_guessed() {
        let sources = scan(&files("struct A; mod m { struct A; }")).unwrap();
        assert!(!sources.types.contains_key("A"));
        assert_eq!(sources.ambiguous, ["A"]);
    }

    #[test]
    fn the_mount_prefix_is_read_from_main() {
        let main = vec![(
            PathBuf::from("src/main.rs"),
            "fn main() { Service::new(\"a\", \"1\").prefix(\"/api\").run(); }".to_owned(),
        )];
        assert_eq!(scan(&main).unwrap().prefix.as_deref(), Some("/api"));
    }

    #[test]
    fn broken_source_names_the_file() {
        let error = scan(&files("fn (")).unwrap_err();
        assert!(error.to_string().contains("src/lib.rs"), "{error}");
    }
}
