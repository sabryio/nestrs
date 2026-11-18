use proc_macro2::TokenStream;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{Expr, Fields, Ident, ItemStruct, LitBool, Token, Type, bracketed};

#[derive(Default)]
struct ModuleArgs {
    global: bool,
    imports: Vec<Expr>,
    providers: Vec<Expr>,
    controllers: Vec<Type>,
    exports: Vec<Type>,
}

impl Parse for ModuleArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut args = Self::default();
        while !input.is_empty() {
            let key: Ident = input.parse()?;

            // `global` is a flag: `#[module(global, ..)]` or `global = true`.
            if key == "global" {
                args.global = if input.peek(Token![=]) {
                    input.parse::<Token![=]>()?;
                    input.parse::<LitBool>()?.value
                } else {
                    true
                };
            } else {
                input.parse::<Token![=]>()?;
                let content;
                bracketed!(content in input);

                match key.to_string().as_str() {
                    "imports" => args.imports = exprs(&content)?,
                    "providers" => args.providers = exprs(&content)?,
                    "controllers" => args.controllers = types(&content)?,
                    "exports" => args.exports = types(&content)?,
                    other => {
                        return Err(syn::Error::new(
                            key.span(),
                            format!(
                                "unknown key `{other}`; expected imports, providers, controllers, exports or global"
                            ),
                        ));
                    }
                }
            }

            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }
        Ok(args)
    }
}

fn types(input: ParseStream) -> syn::Result<Vec<Type>> {
    Ok(Punctuated::<Type, Token![,]>::parse_terminated(input)?
        .into_iter()
        .collect())
}

fn exprs(input: ParseStream) -> syn::Result<Vec<Expr>> {
    Ok(Punctuated::<Expr, Token![,]>::parse_terminated(input)?
        .into_iter()
        .collect())
}

pub(crate) fn expand(args: TokenStream, item: ItemStruct) -> syn::Result<TokenStream> {
    if !matches!(item.fields, Fields::Unit) || !item.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &item,
            "#[module] must be applied to a non-generic unit struct (`pub struct MyModule;`)",
        ));
    }
    let args: ModuleArgs = syn::parse2(args)?;

    let name = &item.ident;
    let label = name.to_string();

    // A bare path is a `Module` type; anything else (`ConfigModule::for_root(..)`)
    // must already evaluate to a `ModuleRef`.
    let imports = args.imports.iter().map(|expr| match expr {
        Expr::Path(path) if path.qself.is_none() => quote!(::purlin::ModuleRef::of::<#path>()),
        other => quote!(#other),
    });
    // Same convention for providers: a bare path is an `#[injectable]` type or a
    // `#[provider]` function (both are `ProviderSource`s); anything else is a `Provider`.
    let providers = args.providers.iter().map(|expr| match expr {
        Expr::Path(path) if path.qself.is_none() => {
            quote!(__providers.extend(<#path as ::purlin::ProviderSource>::providers());)
        }
        other => quote!(__providers.push(#other);),
    });
    let controllers = args
        .controllers
        .iter()
        .map(|controller| quote!(<#controller as ::purlin::Controller>::entry()));
    // Exports accept a type, `dyn Trait` or a port alias; the probe maps all of them
    // to the container key.
    let exports = args.exports.iter().map(|token| {
        quote!({
            use ::purlin::__private::{TokenViaDependency as _, TokenViaType as _};
            (&::purlin::__private::Probe::<#token>::new()).token()
        })
    });
    let global = args.global.then(|| quote!(.global()));

    Ok(quote! {
        #item

        impl ::purlin::Module for #name {
            fn definition() -> ::purlin::ModuleDef {
                ::purlin::ModuleDef::new(#label)
                    #global
                    .imports([#(#imports),*])
                    .providers({
                        let mut __providers: ::std::vec::Vec<::purlin::Provider> = ::std::vec::Vec::new();
                        #(#providers)*
                        __providers
                    })
                    .controllers([#(#controllers),*])
                    .exports([#(#exports),*])
            }
        }
    })
}
