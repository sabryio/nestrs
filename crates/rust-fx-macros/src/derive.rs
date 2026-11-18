use proc_macro2::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Fields};

pub(crate) fn component(input: DeriveInput) -> syn::Result<TokenStream> {
    let name = &input.ident;
    let (impl_generics, type_generics, where_clause) = input.generics.split_for_impl();
    Ok(quote! {
        impl #impl_generics ::fx::Component for #name #type_generics #where_clause {}
    })
}

pub(crate) fn params(input: DeriveInput) -> syn::Result<TokenStream> {
    if !input.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &input.generics,
            "#[derive(fx::Params)] does not support generic structs",
        ));
    }
    let Data::Struct(data) = &input.data else {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "#[derive(fx::Params)] needs a struct",
        ));
    };
    let name = &input.ident;

    let types: Vec<_> = data.fields.iter().map(|field| &field.ty).collect();
    let specs = types.iter().map(|ty| quote!(<#ty as ::fx::Extract>::specs(out);));

    let build = match &data.fields {
        Fields::Named(fields) => {
            let inits = fields.named.iter().map(|field| {
                let (ident, ty) = (&field.ident, &field.ty);
                quote!(#ident: <#ty as ::fx::Extract>::extract(args, ctx)?)
            });
            quote!(Self { #(#inits),* })
        }
        Fields::Unnamed(fields) => {
            let inits = fields.unnamed.iter().map(|field| {
                let ty = &field.ty;
                quote!(<#ty as ::fx::Extract>::extract(args, ctx)?)
            });
            quote!(Self(#(#inits),*))
        }
        Fields::Unit => quote!(Self),
    };

    Ok(quote! {
        impl ::fx::Extract for #name {
            fn specs(out: &mut ::std::vec::Vec<::fx::DepSpec>) {
                #(#specs)*
            }

            fn extract(args: &mut ::fx::Args, ctx: &::fx::Context) -> ::fx::Result<Self> {
                ::std::result::Result::Ok(#build)
            }
        }
    })
}
