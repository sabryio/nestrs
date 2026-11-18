use proc_macro2::TokenStream;
use quote::{quote, quote_spanned};
use syn::spanned::Spanned;
use syn::{FnArg, GenericArgument, ItemFn, Pat, PathArguments, ReturnType, Type};

/// `#[provider]` on a function: its parameters are the dependencies, its (Ok) return
/// type is what it provides. A marker type with the function's name is generated so
/// the function can be listed in `providers = [..]`.
/// `#[provider]` or `#[provider(scope = request | transient)]`.
pub(crate) struct Args {
    scope: Option<syn::Ident>,
}

impl syn::parse::Parse for Args {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let mut scope = None;
        if !input.is_empty() {
            let key: syn::Ident = input.parse()?;
            if key != "scope" {
                return Err(syn::Error::new(
                    key.span(),
                    "unknown option; expected `scope = request | transient`",
                ));
            }
            input.parse::<syn::Token![=]>()?;
            let value: syn::Ident = input.parse()?;
            if value != "request" && value != "transient" && value != "singleton" {
                return Err(syn::Error::new(
                    value.span(),
                    "scope must be `singleton` (default), `request` or `transient`",
                ));
            }
            scope = Some(value);
        }
        Ok(Self { scope })
    }
}

pub(crate) fn expand(args: Args, item: ItemFn) -> syn::Result<TokenStream> {
    let scope = match args.scope.as_ref().map(ToString::to_string).as_deref() {
        Some("request") => quote!(.request()),
        Some("transient") => quote!(.transient()),
        _ => quote!(),
    };
    let sig = &item.sig;
    if !sig.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &sig.generics,
            "#[provider] functions cannot be generic",
        ));
    }
    let output = match &sig.output {
        ReturnType::Type(_, ty) => ty.as_ref(),
        ReturnType::Default => {
            return Err(syn::Error::new_spanned(
                sig,
                "#[provider] functions must return the value they provide",
            ));
        }
    };
    let (provided, fallible) = unwrap_result(output);

    let mut dependency_types = Vec::new();
    let mut names = Vec::new();
    for (index, argument) in sig.inputs.iter().enumerate() {
        let FnArg::Typed(typed) = argument else {
            return Err(syn::Error::new_spanned(
                argument,
                "#[provider] must be a free function, not a method",
            ));
        };
        let _ = matches!(*typed.pat, Pat::Ident(_));
        dependency_types.push(typed.ty.as_ref().clone());
        names.push(quote::format_ident!("dependency_{index}"));
    }

    let function = &sig.ident;
    let vis = &item.vis;
    let doc = format!("Marker that lets [`{function}`] be listed in a module's `providers`.");
    let await_ = sig.asyncness.map(|_| quote!(.await));
    let call = quote!(#function(#(#names),*) #await_);
    let result = if fallible {
        quote!(#call.map_err(::purlin::DiError::custom))
    } else {
        quote!(::std::result::Result::<_, ::purlin::DiError>::Ok(#call))
    };
    let tokens = dependency_types
        .iter()
        .map(|ty| quote_spanned!(ty.span()=> <#ty as ::purlin::Dependency>::token()));
    let resolves = dependency_types
        .iter()
        .zip(&names)
        .map(|(ty, name)| quote_spanned!(ty.span()=> let #name = <#ty as ::purlin::Dependency>::resolve(container)?;));

    Ok(quote! {
        #item

        #[doc = #doc]
        #[allow(non_camel_case_types)]
        #vis struct #function {}

        impl ::purlin::ProviderSource for #function {
            fn providers() -> ::std::vec::Vec<::purlin::Provider> {
                use ::purlin::__private::{OutputViaHandle as _, OutputViaValue as _};
                use ::purlin::__private::{DestroySkipped as _, InitSkipped as _};
                use ::purlin::__private::{DestroyViaTrait as _, InitViaTrait as _};
                let dependencies: ::std::vec::Vec<::purlin::Token> = ::std::vec![#(#tokens),*];
                let provider = (&::purlin::__private::Probe::<#provided>::new()).provide_with(
                    dependencies,
                    |container| ::std::boxed::Box::pin(async move {
                        #(#resolves)*
                        #result
                    }),
                );
                // A provided value that implements the lifecycle traits gets its hooks.
                let probe = ::purlin::__private::Probe::<#provided>::new();
                let provider = (&probe).attach_init(provider);
                ::std::vec![(&probe).attach_destroy(provider) #scope]
            }
        }
    })
}

/// `Result<T, E>` -> (`T`, true); anything else -> (itself, false).
fn unwrap_result(ty: &Type) -> (Type, bool) {
    if let Type::Path(path) = ty
        && let Some(segment) = path.path.segments.last()
        && segment.ident == "Result"
        && let PathArguments::AngleBracketed(arguments) = &segment.arguments
        && let Some(GenericArgument::Type(ok)) = arguments.args.first()
    {
        return (ok.clone(), true);
    }
    (ty.clone(), false)
}
