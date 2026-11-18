use proc_macro2::TokenStream;
use quote::{quote, quote_spanned};
use syn::spanned::Spanned;

use crate::ir::{Module, Provider};
use crate::pipeline::Context;

pub(crate) fn generate(ir: &Module, cx: &Context) -> TokenStream {
    let fx = &cx.fx;
    let (item, ident, name) = (&ir.item, &ir.item.ident, &ir.name);

    let imports = ir.imports.iter().map(|ty| quote!(.import(#fx::ModuleRef::of::<#ty>())));
    let providers = ir.providers.iter().map(|provider| match provider {
        Provider::Component(path) => quote_spanned!(path.span()=> .component::<#path>()),
        Provider::Constructor(expr) => quote_spanned!(expr.span()=> .constructor(#expr)),
    });
    let invokes = ir
        .invokes
        .iter()
        .map(|expr| quote_spanned!(expr.span()=> .invoke(#expr)));
    let decorators = ir
        .decorators
        .iter()
        .map(|expr| quote_spanned!(expr.span()=> .decorate(#expr)));
    let exports = ir.exports.iter().map(|export| {
        let ty = &export.ty;
        match &export.name {
            Some(name) => quote!(.export(#fx::Export::named::<#ty>(#name))),
            None => quote!(.export(#fx::Export::of::<#ty>())),
        }
    });

    quote! {
        #item

        impl #fx::Module for #ident {
            fn definition() -> #fx::ModuleDef {
                #fx::ModuleDef::new(#name)
                    #(#imports)*
                    .options(
                        #fx::Options::new()
                            #(#providers)*
                            #(#invokes)*
                            #(#decorators)*
                    )
                    #(#exports)*
            }
        }
    }
}
