use crate::ast::bind::BindAst;
use crate::diagnostics::Diagnostics;
use crate::ir;

pub(crate) fn analyze(ast: &BindAst, diagnostics: &mut Diagnostics) -> ir::Binding {
    let item = &ast.item;
    let placement = super::placement(&ast.options, &item.self_ty, "binding", diagnostics);

    if !item.generics.params.is_empty() {
        diagnostics.error(
            &item.generics,
            "#[fx::bind] does not support generic impls; bind each concrete type separately",
        );
    }

    let interface = match &item.trait_ {
        Some((path, _)) => path.clone(),
        None => {
            diagnostics.error(
                &item.self_ty,
                "#[fx::bind] goes on a trait impl, `impl Trait for Type { .. }`: that is what \
                 names the interface",
            );
            // Any placeholder will do; the IR is discarded when errors were reported.
            syn::parse_quote!(Interface)
        }
    };
    ir::Binding {
        item: item.clone(),
        component: (*item.self_ty).clone(),
        interface,
        placement,
    }
}
