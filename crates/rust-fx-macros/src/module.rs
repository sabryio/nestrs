use proc_macro2::TokenStream;
use quote::{quote, quote_spanned};
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::{Expr, Fields, Ident, ItemStruct, LitStr, Token, Type, bracketed};

use crate::util::looks_like_type;

struct Export {
    ty: Type,
    name: Option<LitStr>,
}

impl Parse for Export {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let ty = input.parse()?;
        let name = if input.peek(Token![as]) {
            input.parse::<Token![as]>()?;
            Some(input.parse()?)
        } else {
            None
        };
        Ok(Self { ty, name })
    }
}

#[derive(Default)]
struct Args {
    name: Option<LitStr>,
    imports: Vec<Type>,
    providers: Vec<Expr>,
    invokes: Vec<Expr>,
    decorators: Vec<Expr>,
    exports: Vec<Export>,
}

fn list<T: Parse>(input: ParseStream) -> syn::Result<Vec<T>> {
    let content;
    bracketed!(content in input);
    Ok(Punctuated::<T, Token![,]>::parse_terminated(&content)?
        .into_iter()
        .collect())
}

impl Parse for Args {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut args = Self::default();
        while !input.is_empty() {
            let key: Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            match key.to_string().as_str() {
                "name" => args.name = Some(input.parse()?),
                "imports" => args.imports = list(input)?,
                "providers" => args.providers = list(input)?,
                "invokes" => args.invokes = list(input)?,
                "decorators" => args.decorators = list(input)?,
                "exports" => args.exports = list(input)?,
                other => {
                    return Err(syn::Error::new(
                        key.span(),
                        format!(
                            "unknown key `{other}`; expected name, imports, providers, invokes, decorators or exports"
                        ),
                    ));
                }
            }
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }
        Ok(args)
    }
}

/// A bare path to a type registers a component; anything else is one constructor.
fn as_component(expr: &Expr) -> Option<&syn::Path> {
    match expr {
        Expr::Path(path) if path.qself.is_none() && looks_like_type(&path.path) => Some(&path.path),
        _ => None,
    }
}

pub(crate) fn expand(args: TokenStream, item: ItemStruct) -> syn::Result<TokenStream> {
    if !matches!(item.fields, Fields::Unit) || !item.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &item,
            "#[fx::module] must be applied to a non-generic unit struct (`pub struct MyModule;`)",
        ));
    }
    let args: Args = syn::parse2(args)?;
    let ident = &item.ident;
    let name = args.name.map_or_else(|| ident.to_string(), |lit| lit.value());

    let imports = args
        .imports
        .iter()
        .map(|ty| quote!(.import(::fx::ModuleRef::of::<#ty>())));

    let providers = args.providers.iter().map(|expr| match as_component(expr) {
        Some(path) => quote_spanned!(expr.span()=> .component::<#path>()),
        None => quote_spanned!(expr.span()=> .provide(#expr)),
    });

    let functions = |exprs: &[Expr], method: &str| -> syn::Result<Vec<TokenStream>> {
        let method = Ident::new(method, proc_macro2::Span::call_site());
        exprs
            .iter()
            .map(|expr| {
                if as_component(expr).is_some() {
                    return Err(syn::Error::new_spanned(
                        expr,
                        "list components under `providers`; they register their own invokes and decorators",
                    ));
                }
                Ok(quote_spanned!(expr.span()=> .#method(#expr)))
            })
            .collect()
    };
    let invokes = functions(&args.invokes, "invoke")?;
    let decorators = functions(&args.decorators, "decorate")?;

    let exports = args.exports.iter().map(|export| {
        let ty = &export.ty;
        match &export.name {
            Some(name) => quote!(.export(::fx::Export::named::<#ty>(#name))),
            None => quote!(.export(::fx::Export::of::<#ty>())),
        }
    });

    Ok(quote! {
        #item

        impl ::fx::Module for #ident {
            fn definition() -> ::fx::ModuleDef {
                ::fx::ModuleDef::new(#name)
                    #(#imports)*
                    .options(
                        ::fx::Options::new()
                            #(#providers)*
                            #(#invokes)*
                            #(#decorators)*
                    )
                    #(#exports)*
            }
        }
    })
}
