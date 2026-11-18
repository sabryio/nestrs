use proc_macro2::TokenStream;
use quote::quote;

use crate::ir::{ComponentMarker, Params, ParamsShape};
use crate::pipeline::Context;

pub(crate) fn component(ir: &ComponentMarker, cx: &Context) -> TokenStream {
    let fx = &cx.fx;
    let name = &ir.input.ident;
    let (impl_generics, type_generics, where_clause) = ir.input.generics.split_for_impl();
    quote! {
        impl #impl_generics #fx::Component for #name #type_generics #where_clause {}
    }
}

pub(crate) fn params(ir: &Params, cx: &Context) -> TokenStream {
    let fx = &cx.fx;
    let name = &ir.name;
    let types: Vec<&syn::Type> = match &ir.shape {
        ParamsShape::Named(fields) => fields.iter().map(|(_, ty)| ty).collect(),
        ParamsShape::Tuple(types) => types.iter().collect(),
        ParamsShape::Unit => Vec::new(),
    };
    let specs = types.iter().map(|ty| quote!(<#ty as #fx::Extract>::specs(out);));
    let build = match &ir.shape {
        ParamsShape::Named(fields) => {
            let inits = fields
                .iter()
                .map(|(ident, ty)| quote!(#ident: <#ty as #fx::Extract>::extract(args, ctx)?));
            quote!(Self { #(#inits),* })
        }
        ParamsShape::Tuple(types) => {
            let inits = types
                .iter()
                .map(|ty| quote!(<#ty as #fx::Extract>::extract(args, ctx)?));
            quote!(Self(#(#inits),*))
        }
        ParamsShape::Unit => quote!(Self),
    };
    quote! {
        impl #fx::Extract for #name {
            fn specs(out: &mut ::std::vec::Vec<#fx::DepSpec>) {
                #(#specs)*
            }

            fn extract(args: &mut #fx::Args, ctx: &#fx::Context) -> #fx::Result<Self> {
                ::std::result::Result::Ok(#build)
            }
        }
    }
}
