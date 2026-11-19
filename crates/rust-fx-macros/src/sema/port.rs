use syn::{TypeParamBound, parse_quote};

use crate::ast::port::PortAst;
use crate::diagnostics::Diagnostics;
use crate::ir;

fn has_bound(item: &syn::ItemTrait, name: &str) -> bool {
    item.supertraits.iter().any(|bound| match bound {
        TypeParamBound::Trait(bound) => bound.path.segments.last().is_some_and(|segment| segment.ident == name),
        _ => false,
    })
}

pub(crate) fn analyze(ast: &PortAst, diagnostics: &mut Diagnostics) -> ir::Port {
    let mut item = ast.item.clone();

    if !item.generics.params.is_empty() {
        diagnostics.error(
            &item.generics,
            "#[fx::port] does not support generic traits: a port is one interface, so name each \
             instantiation with its own trait",
        );
    }

    let alias = ast
        .alias
        .clone()
        .unwrap_or_else(|| syn::Ident::new(&format!("{}Port", item.ident), item.ident.span()));
    if alias == item.ident {
        diagnostics.error(&alias, "the alias must differ from the trait's own name");
    }

    // Values are shared across tasks, so a port must be `Send + Sync`. Say it once here instead
    // of making every trait declaration repeat it.
    if !has_bound(&item, "Send") {
        item.supertraits.push(parse_quote!(::core::marker::Send));
    }
    if !has_bound(&item, "Sync") {
        item.supertraits.push(parse_quote!(::core::marker::Sync));
    }

    ir::Port {
        vis: item.vis.clone(),
        trait_ident: item.ident.clone(),
        item,
        alias,
    }
}
