use std::collections::HashSet;

use quote::ToTokens;
use syn::Expr;

use crate::ast::module::{Entry, ModuleAst, Value};
use crate::diagnostics::Diagnostics;
use crate::ir::{self, Export, Provider};
use crate::support::syntax::looks_like_type;

/// A bare path to a type registers a component; anything else is one constructor.
fn component_path(expr: &Expr) -> Option<&syn::Path> {
    match expr {
        Expr::Path(path) if path.qself.is_none() && looks_like_type(&path.path) => Some(&path.path),
        _ => None,
    }
}

fn key_name(entry: &Entry) -> String {
    entry.key.to_string()
}

pub(crate) fn analyze(ast: &ModuleAst, diagnostics: &mut Diagnostics) -> ir::Module {
    let item = &ast.item;
    if !matches!(item.fields, syn::Fields::Unit) || !item.generics.params.is_empty() {
        diagnostics.error(
            item,
            "#[fx::module] must be applied to a non-generic unit struct (`pub struct MyModule;`)",
        );
    }

    let mut module = ir::Module {
        item: item.clone(),
        name: item.ident.to_string(),
        imports: Vec::new(),
        providers: Vec::new(),
        invokes: Vec::new(),
        decorators: Vec::new(),
        exports: Vec::new(),
    };

    let mut seen = HashSet::new();
    for entry in &ast.entries {
        if !seen.insert(key_name(entry)) {
            diagnostics.error(&entry.key, format!("`{}` is given twice; merge the lists", entry.key));
            continue;
        }
        match &entry.value {
            Value::Name(name) => module.name = name.value(),
            Value::Imports(types) => module.imports.extend(types.iter().cloned()),
            Value::Providers(exprs) => {
                let mut listed = HashSet::new();
                for expr in exprs {
                    if !listed.insert(expr.to_token_stream().to_string()) {
                        diagnostics.error(expr, "listed twice; a provider can only be registered once per module");
                        continue;
                    }
                    module.providers.push(match component_path(expr) {
                        Some(path) => Provider::Component(path.clone()),
                        None => Provider::Constructor(expr.clone()),
                    });
                }
            }
            Value::Invokes(exprs) => module.invokes.extend(functions(exprs, diagnostics)),
            Value::Decorators(exprs) => module.decorators.extend(functions(exprs, diagnostics)),
            Value::Exports(exports) => {
                let mut listed = HashSet::new();
                for export in exports {
                    let identity = (
                        export.ty.to_token_stream().to_string(),
                        export.name.as_ref().map(syn::LitStr::value),
                    );
                    if !listed.insert(identity) {
                        diagnostics.error(&export.ty, "exported twice");
                        continue;
                    }
                    module.exports.push(Export {
                        ty: export.ty.clone(),
                        name: export.name.clone(),
                    });
                }
            }
        }
    }
    module
}

fn functions(exprs: &[Expr], diagnostics: &mut Diagnostics) -> Vec<Expr> {
    exprs
        .iter()
        .filter(|expr| {
            if component_path(expr).is_some() {
                diagnostics.error(
                    expr,
                    "list components under `providers`; they register their own invokes and decorators",
                );
                return false;
            }
            true
        })
        .cloned()
        .collect()
}
