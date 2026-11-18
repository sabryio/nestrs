//! The macro compiler pipeline.
//!
//! Every public macro goes through the same four stages, and each stage has one job:
//!
//! ```text
//!   tokens ──parse──▶ AST ──analyze──▶ IR ──generate──▶ tokens
//!            syntax    │    semantics    │    codegen
//!            only      └─ Diagnostics    └─ no validation, no guessing
//! ```
//!
//! - **parse** (`ast/`) turns tokens into a syntax tree of what was *written*. It rejects
//!   what cannot be parsed and nothing else.
//! - **analyze** (`sema/`) applies the rules ("a hook needs a constructor", "no duplicate
//!   keys"), reports *every* violation through [`Diagnostics`], and resolves defaults into
//!   the IR.
//! - **IR** (`ir.rs`) is the vocabulary shared by all macros: constructors, hooks,
//!   placements, exports. It only holds valid, fully resolved data.
//! - **generate** (`codegen/`) turns the IR into Rust. It cannot fail: anything it needs
//!   was guaranteed by analysis.
//!
//! A new macro is a new [`Macro`] implementation; a new rule is one function in `sema/`;
//! a change to the generated code touches only `codegen/`.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;

use crate::diagnostics::Diagnostics;

/// What code generation needs to know about its surroundings.
pub(crate) struct Context {
    /// The path under which generated code reaches the `fx` crate.
    pub(crate) fx: TokenStream2,
}

/// One procedural macro, as the four stages of the pipeline.
pub(crate) trait Macro {
    /// What was written.
    type Ast;
    /// What it means.
    type Ir;

    /// Syntax only. `args` is the attribute's argument list (empty for derives).
    fn parse(args: TokenStream2, item: TokenStream2) -> syn::Result<Self::Ast>;

    /// Semantics: report every rule violation and resolve the AST into the IR. The IR may
    /// be incomplete when errors were reported; it is then thrown away.
    fn analyze(ast: &Self::Ast, diagnostics: &mut Diagnostics) -> Self::Ir;

    /// Code generation from a valid IR.
    fn generate(ir: &Self::Ir, cx: &Context) -> TokenStream2;

    /// The item as the user wrote it (minus our attributes), emitted next to the errors so
    /// that one mistake does not also hide the item and cause a cascade of "not found".
    fn passthrough(ast: &Self::Ast) -> TokenStream2;
}

/// Runs the pipeline on token streams.
pub(crate) fn expand<M: Macro>(args: TokenStream2, item: TokenStream2, cx: &Context) -> TokenStream2 {
    let ast = match M::parse(args, item) {
        Ok(ast) => ast,
        Err(error) => return error.to_compile_error(),
    };
    let mut diagnostics = Diagnostics::new();
    let ir = M::analyze(&ast, &mut diagnostics);
    match diagnostics.into_tokens() {
        None => M::generate(&ir, cx),
        Some(mut errors) => {
            errors.extend(M::passthrough(&ast));
            errors
        }
    }
}

/// The entry point of every public macro: resolves the context and runs the pipeline.
pub(crate) fn run<M: Macro>(args: TokenStream, item: TokenStream) -> TokenStream {
    let cx = Context {
        fx: crate::support::paths::fx_path(),
    };
    expand::<M>(args.into(), item.into(), &cx).into()
}
