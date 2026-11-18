//! The five macros, each as one implementation of [`Macro`]: which parser, which analysis,
//! which generator. All behaviour lives in the stage modules.

use proc_macro2::TokenStream;
use quote::ToTokens;
use syn::DeriveInput;

use crate::diagnostics::Diagnostics;
use crate::pipeline::{Context, Macro};
use crate::{ast, codegen, ir, sema};

/// `#[fx::component]`
pub(crate) struct ComponentMacro;

impl Macro for ComponentMacro {
    type Ast = ast::component::ComponentAst;
    type Ir = ir::Component;

    fn parse(args: TokenStream, item: TokenStream) -> syn::Result<Self::Ast> {
        ast::component::parse(args, item)
    }
    fn analyze(ast: &Self::Ast, diagnostics: &mut Diagnostics) -> Self::Ir {
        sema::component::analyze(ast, diagnostics)
    }
    fn generate(ir: &Self::Ir, cx: &Context) -> TokenStream {
        codegen::component::generate(ir, cx)
    }
    fn passthrough(ast: &Self::Ast) -> TokenStream {
        ast.item.to_token_stream()
    }
}

/// `#[fx::bind]`
pub(crate) struct BindMacro;

impl Macro for BindMacro {
    type Ast = ast::bind::BindAst;
    type Ir = ir::Binding;

    fn parse(args: TokenStream, item: TokenStream) -> syn::Result<Self::Ast> {
        ast::bind::parse(args, item)
    }
    fn analyze(ast: &Self::Ast, diagnostics: &mut Diagnostics) -> Self::Ir {
        sema::bind::analyze(ast, diagnostics)
    }
    fn generate(ir: &Self::Ir, cx: &Context) -> TokenStream {
        codegen::bind::generate(ir, cx)
    }
    fn passthrough(ast: &Self::Ast) -> TokenStream {
        ast.item.to_token_stream()
    }
}

/// `#[fx::module(..)]`
pub(crate) struct ModuleMacro;

impl Macro for ModuleMacro {
    type Ast = ast::module::ModuleAst;
    type Ir = ir::Module;

    fn parse(args: TokenStream, item: TokenStream) -> syn::Result<Self::Ast> {
        ast::module::parse(args, item)
    }
    fn analyze(ast: &Self::Ast, diagnostics: &mut Diagnostics) -> Self::Ir {
        sema::module::analyze(ast, diagnostics)
    }
    fn generate(ir: &Self::Ir, cx: &Context) -> TokenStream {
        codegen::module::generate(ir, cx)
    }
    fn passthrough(ast: &Self::Ast) -> TokenStream {
        ast.item.to_token_stream()
    }
}

/// `#[derive(fx::Params)]`
pub(crate) struct ParamsMacro;

impl Macro for ParamsMacro {
    type Ast = DeriveInput;
    type Ir = ir::Params;

    fn parse(args: TokenStream, item: TokenStream) -> syn::Result<Self::Ast> {
        ast::derive::parse(args, item)
    }
    fn analyze(ast: &Self::Ast, diagnostics: &mut Diagnostics) -> Self::Ir {
        sema::derive::params(ast, diagnostics)
    }
    fn generate(ir: &Self::Ir, cx: &Context) -> TokenStream {
        codegen::derive::params(ir, cx)
    }
    fn passthrough(_: &Self::Ast) -> TokenStream {
        // A derive never re-emits its input.
        TokenStream::new()
    }
}

/// `#[derive(fx::Component)]`
pub(crate) struct ComponentDerive;

impl Macro for ComponentDerive {
    type Ast = DeriveInput;
    type Ir = ir::ComponentMarker;

    fn parse(args: TokenStream, item: TokenStream) -> syn::Result<Self::Ast> {
        ast::derive::parse(args, item)
    }
    fn analyze(ast: &Self::Ast, diagnostics: &mut Diagnostics) -> Self::Ir {
        sema::derive::component(ast, diagnostics)
    }
    fn generate(ir: &Self::Ir, cx: &Context) -> TokenStream {
        codegen::derive::component(ir, cx)
    }
    fn passthrough(_: &Self::Ast) -> TokenStream {
        TokenStream::new()
    }
}
