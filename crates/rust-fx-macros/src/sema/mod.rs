//! Stage 2, analysis: the rules, and the resolution of defaults into the IR.
//!
//! Every function here reports *all* problems it finds through [`Diagnostics`] and still
//! returns an IR, so one compile shows every mistake.

pub(crate) mod bind;
pub(crate) mod component;
pub(crate) mod derive;
pub(crate) mod main;
pub(crate) mod module;
pub(crate) mod port;

use crate::ast::PlacementOptions;
use crate::diagnostics::Diagnostics;
use crate::ir::Placement;

/// Resolves placement options into a [`Placement`], rejecting contradictions.
pub(crate) fn placement(
    options: &PlacementOptions,
    at: &impl quote::ToTokens,
    what: &str,
    diagnostics: &mut Diagnostics,
) -> Placement {
    match (&options.name, &options.group) {
        (Some(_), Some(_)) => {
            diagnostics.error(at, format!("a {what} is either named or in a group, not both"));
            Placement::Value
        }
        (Some(name), None) => Placement::Named(name.clone()),
        (None, Some(group)) => Placement::Group(group.clone()),
        (None, None) => Placement::Value,
    }
}
