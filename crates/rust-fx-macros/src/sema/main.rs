use crate::ast::main::MainAst;
use crate::diagnostics::Diagnostics;
use crate::ir;

pub(crate) fn analyze(ast: &MainAst, diagnostics: &mut Diagnostics) -> ir::Entry {
    let item = &ast.item;

    if item.sig.asyncness.is_none() {
        diagnostics.error(
            item.sig.fn_token,
            "#[fx::main] needs an `async fn`: it starts the async runtime and runs the function on it",
        );
    }
    if !item.sig.inputs.is_empty() {
        diagnostics.error(&item.sig.inputs, "the function marked #[fx::main] takes no arguments");
    }
    for option in &ast.options {
        if option.path().is_ident("crate") {
            diagnostics.error(
                option,
                "`crate = ..` is set by #[fx::main] itself (the runtime comes from `fx::tokio`)",
            );
        }
    }

    ir::Entry {
        item: item.clone(),
        options: ast.options.iter().cloned().collect(),
    }
}
