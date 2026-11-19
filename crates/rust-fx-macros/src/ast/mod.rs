//! Stage 1, parsing: tokens into a syntax tree of what was written.
//!
//! Nothing here judges whether the input *makes sense*; that is analysis. Only input that
//! cannot be read at all is an error.

pub(crate) mod bind;
pub(crate) mod component;
pub(crate) mod derive;
pub(crate) mod main;
pub(crate) mod module;
pub(crate) mod port;

use syn::{LitStr, Meta, Token};

/// The placement options shared by `#[fx::constructor(..)]` and `#[fx::bind(..)]`:
/// `name = ".."`, `group` and `group = ".."`.
#[derive(Default)]
pub(crate) struct PlacementOptions {
    pub(crate) name: Option<LitStr>,
    /// `Some(None)` is the unnamed group, `Some(Some(..))` a named one.
    pub(crate) group: Option<Option<LitStr>>,
}

impl PlacementOptions {
    /// Parses the options inside `meta`'s parentheses; `what` names the attribute in errors.
    pub(crate) fn from_meta(meta: &Meta, what: &str) -> syn::Result<Self> {
        let mut options = Self::default();
        let Meta::List(list) = meta else {
            return Ok(options);
        };
        list.parse_nested_meta(|nested| {
            if nested.path.is_ident("name") {
                options.name = Some(nested.value()?.parse()?);
            } else if nested.path.is_ident("group") {
                options.group = Some(if nested.input.peek(Token![=]) {
                    Some(nested.value()?.parse()?)
                } else {
                    None
                });
            } else if nested.path.is_ident("as") {
                return Err(nested
                    .error("`as = ..` is gone: put `#[fx::bind]` on the trait impl (`impl Trait for Type`) instead"));
            } else {
                return Err(nested.error(format!(
                    "`#[fx::{what}]` accepts `name = \"..\"`, `group` or `group = \"..\"`"
                )));
            }
            Ok(())
        })?;
        Ok(options)
    }
}
