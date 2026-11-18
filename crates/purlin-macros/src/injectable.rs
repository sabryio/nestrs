use proc_macro2::TokenStream;
use quote::{quote, quote_spanned};
use syn::parse::{Parse, ParseStream};
use syn::spanned::Spanned;
use syn::{Attribute, Fields, Ident, ItemStruct, Token, Type};

/// `#[injectable]` or `#[injectable(provides = SomePort)]`.
#[derive(Default)]
pub(crate) struct Args {
    provides: Option<Type>,
    contributes: Option<Type>,
    scope: Option<Ident>,
}

impl Parse for Args {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut args = Self::default();
        while !input.is_empty() {
            let key: Ident = input.parse()?;
            if key != "provides" && key != "contributes" && key != "scope" {
                return Err(syn::Error::new(
                    key.span(),
                    "unknown option; expected `provides = ..`, `contributes = ..` or `scope = request | transient`",
                ));
            }
            input.parse::<Token![=]>()?;
            match key.to_string().as_str() {
                "provides" => args.provides = Some(input.parse()?),
                "contributes" => args.contributes = Some(input.parse()?),
                "scope" => {
                    let scope: Ident = input.parse()?;
                    if scope != "request" && scope != "transient" && scope != "singleton" {
                        return Err(syn::Error::new(
                            scope.span(),
                            "scope must be `singleton` (default), `request` or `transient`",
                        ));
                    }
                    args.scope = Some(scope);
                }
                _ => {
                    return Err(syn::Error::new(
                        key.span(),
                        "unknown option; expected `provides = ..`, `contributes = ..` or `scope = request | transient`",
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

pub(crate) fn expand(args: Args, mut item: ItemStruct) -> syn::Result<TokenStream> {
    if !item.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &item.generics,
            "#[injectable] does not support generic structs",
        ));
    }

    let mut dependencies = Vec::new();
    let mut initialisers = Vec::new();

    match &mut item.fields {
        Fields::Named(fields) => {
            for field in &mut fields.named {
                let name = field.ident.clone().expect("named fields have identifiers");

                if take_inject_default(&mut field.attrs)? {
                    initialisers.push(quote!(#name: ::std::default::Default::default()));
                    continue;
                }

                // Any `Dependency` works: `Arc<T>` or a port alias such as
                // `UserRepositoryPort`. Resolution is driven by the type.
                let ty = &field.ty;
                dependencies.push(quote_spanned!(ty.span()=> <#ty as ::purlin::Dependency>::token()));
                initialisers
                    .push(quote_spanned!(ty.span()=> #name: <#ty as ::purlin::Dependency>::resolve(_container)?));
            }
        }
        Fields::Unit => {}
        Fields::Unnamed(fields) => {
            return Err(syn::Error::new_spanned(
                fields,
                "#[injectable] needs named fields (or a unit struct)",
            ));
        }
    }

    let name = &item.ident;
    let scope = match args.scope.as_ref().map(ToString::to_string).as_deref() {
        Some("request") => quote!(.request()),
        Some("transient") => quote!(.transient()),
        _ => quote!(),
    };
    if let (Some(port), true) = (&args.contributes, args.provides.is_some() || !scope.is_empty()) {
        return Err(syn::Error::new_spanned(
            port,
            "`contributes` cannot be combined with `provides` or a non-singleton `scope`",
        ));
    }
    let finish = match &args.provides {
        None if args.contributes.is_some() => {
            let port = args.contributes.as_ref();
            quote! {
                ::std::vec![::purlin::Provider::contribute::<<#port as ::purlin::Dependency>::Target, Self>(
                    |instance: ::std::sync::Arc<Self>| -> #port { instance },
                )]
            }
        }
        // A scoped adapter is built per request (or per resolution) and carries no
        // lifecycle hooks, which only singletons have.
        Some(port) if !scope.is_empty() => quote! {
            ::std::vec![::purlin::Provider::bind::<<#port as ::purlin::Dependency>::Target, Self>(
                |instance: ::std::sync::Arc<Self>| -> #port { instance },
            ) #scope]
        },
        Some(port) => quote! {
            ::purlin::__private::register_bound::<#port, Self>(
                |instance: ::std::sync::Arc<Self>| -> #port { instance },
                hooked,
                provider,
            )
        },
        None => quote!(::std::vec![provider]),
    };
    // Probing must happen here, where `Self` is concrete, so hooks are found.
    let register = quote! {
        {
            use ::purlin::__private::{DestroySkipped as _, InitSkipped as _};
            use ::purlin::__private::{DestroyViaTrait as _, InitViaTrait as _};
            let probe = ::purlin::__private::Probe::<Self>::new();
            let hooked = (&probe).has_init() || (&probe).has_destroy();
            let provider = ::purlin::Provider::of::<Self>() #scope;
            let provider = (&probe).attach_init(provider);
            let provider = (&probe).attach_destroy(provider);
            #finish
        }
    };
    Ok(quote! {
        #item

        impl ::purlin::Injectable for #name {
            fn dependencies() -> ::std::vec::Vec<::purlin::Token> {
                ::std::vec![#(#dependencies),*]
            }

            fn register() -> ::std::vec::Vec<::purlin::Provider> {
                #register
            }

            fn build(
                _container: &::purlin::Container,
            ) -> impl ::std::future::Future<Output = ::std::result::Result<Self, ::purlin::DiError>>
                   + ::std::marker::Send {
                async move {
                    ::std::result::Result::<Self, ::purlin::DiError>::Ok(Self {
                        #(#initialisers),*
                    })
                }
            }
        }
    })
}

/// Removes `#[inject(..)]` from a field, returning whether it was `#[inject(default)]`.
fn take_inject_default(attrs: &mut Vec<Attribute>) -> syn::Result<bool> {
    let mut found = false;
    let mut failure = None;
    attrs.retain(|attr| {
        if !attr.path().is_ident("inject") {
            return true;
        }
        match attr.parse_args::<Ident>() {
            Ok(option) if option == "default" => found = true,
            Ok(option) => {
                failure = Some(syn::Error::new(
                    option.span(),
                    "unknown option; the only supported form is `#[inject(default)]`",
                ));
            }
            Err(error) => failure = Some(error),
        }
        false
    });
    match failure {
        Some(error) => Err(error),
        None => Ok(found),
    }
}
