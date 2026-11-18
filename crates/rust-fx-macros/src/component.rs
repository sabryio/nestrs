use proc_macro2::{Span, TokenStream};
use quote::{quote, quote_spanned};
use syn::spanned::Spanned;
use syn::{FnArg, Ident, ImplItem, ImplItemFn, ItemImpl, LitStr, Meta, ReceiverKind, Type};

use crate::util::member_attr;

#[derive(Default)]
struct ProvideOptions {
    interface: Option<Type>,
    name: Option<LitStr>,
    /// `Some(None)` is the unnamed group, `Some(Some(..))` a named one.
    group: Option<Option<LitStr>>,
}

struct Provide {
    method: Ident,
    options: ProvideOptions,
}

struct Hook {
    method: Ident,
    is_async: bool,
}

fn parse_provide(attr: &syn::Attribute) -> syn::Result<ProvideOptions> {
    let mut options = ProvideOptions::default();
    if matches!(attr.meta, Meta::Path(_)) {
        return Ok(options);
    }
    attr.parse_nested_meta(|meta| {
        if meta.path.is_ident("as") {
            options.interface = Some(meta.value()?.parse()?);
        } else if meta.path.is_ident("name") {
            options.name = Some(meta.value()?.parse()?);
        } else if meta.path.is_ident("group") {
            options.group = Some(if meta.input.peek(syn::Token![=]) {
                Some(meta.value()?.parse()?)
            } else {
                None
            });
        } else {
            return Err(meta.error("expected `as = <type>`, `name = \"..\"` or `group`"));
        }
        Ok(())
    })?;
    if options.name.is_some() && options.group.is_some() {
        return Err(syn::Error::new_spanned(
            attr,
            "a provider is either named or in a group, not both",
        ));
    }
    Ok(options)
}

fn has_receiver(method: &ImplItemFn) -> bool {
    matches!(method.sig.inputs.first(), Some(FnArg::Receiver(_)))
}

fn require_associated(method: &ImplItemFn, what: &str) -> syn::Result<()> {
    if has_receiver(method) {
        return Err(syn::Error::new(
            method.sig.span(),
            format!("`#[fx::{what}]` functions take their dependencies as parameters, not `self`"),
        ));
    }
    Ok(())
}

fn require_borrowed_self(method: &ImplItemFn, what: &str) -> syn::Result<()> {
    match method.sig.inputs.first() {
        Some(FnArg::Receiver(receiver)) if matches!(&receiver.kind, ReceiverKind::Reference(_, _, None)) => Ok(()),
        _ => Err(syn::Error::new(
            method.sig.span(),
            format!("`#[fx::{what}]` methods must take `&self` (the built value is shared)"),
        )),
    }
}

pub(crate) fn expand(mut item: ItemImpl) -> syn::Result<TokenStream> {
    if let Some((path, _)) = &item.trait_ {
        return Err(syn::Error::new_spanned(
            path,
            "#[fx::component] goes on an inherent `impl Type { .. }` block, not a trait impl",
        ));
    }
    if !item.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &item.generics,
            "#[fx::component] does not support generic impls; implement `fx::Component` by hand",
        ));
    }

    let mut provides = Vec::new();
    let mut starts = Vec::new();
    let mut stops = Vec::new();
    let mut invokes = Vec::new();
    let mut decorates = Vec::new();

    for member in &mut item.items {
        let ImplItem::Fn(method) = member else { continue };

        let mut kept = Vec::new();
        let mut found = Vec::new();
        for attr in std::mem::take(&mut method.attrs) {
            match member_attr(&attr) {
                Some(kind) => found.push((kind, attr)),
                None => kept.push(attr),
            }
        }
        method.attrs = kept;

        for (kind, attr) in found {
            let ident = method.sig.ident.clone();
            match kind {
                "provide" => {
                    require_associated(method, "provide")?;
                    provides.push(Provide {
                        method: ident,
                        options: parse_provide(&attr)?,
                    });
                }
                "on_start" => {
                    require_borrowed_self(method, "on_start")?;
                    starts.push(Hook {
                        method: ident,
                        is_async: method.sig.asyncness.is_some(),
                    });
                }
                "on_stop" => {
                    require_borrowed_self(method, "on_stop")?;
                    stops.push(Hook {
                        method: ident,
                        is_async: method.sig.asyncness.is_some(),
                    });
                }
                "invoke" => {
                    require_associated(method, "invoke")?;
                    invokes.push(ident);
                }
                _ => {
                    require_associated(method, "decorate")?;
                    decorates.push(ident);
                }
            }
        }
    }

    if provides.is_empty() && (!starts.is_empty() || !stops.is_empty()) {
        return Err(syn::Error::new(
            Span::call_site(),
            "`#[fx::on_start]` / `#[fx::on_stop]` need a `#[fx::provide]` constructor in the same impl block",
        ));
    }

    let self_ty = item.self_ty.clone();

    let hook = |hook: &Hook, register: Ident| {
        let method = &hook.method;
        let body = if hook.is_async {
            quote!(async move { __this.#method().await })
        } else {
            quote!(async move { __this.#method() })
        };
        quote!(.#register(|__this: ::std::sync::Arc<#self_ty>| #body))
    };
    let hooks: Vec<TokenStream> = starts
        .iter()
        .map(|h| hook(h, Ident::new("on_start", Span::call_site())))
        .chain(stops.iter().map(|h| hook(h, Ident::new("on_stop", Span::call_site()))))
        .collect();

    let providers = provides.iter().map(|provide| {
        let method = &provide.method;
        let span = method.span();
        let bind = provide
            .options
            .interface
            .as_ref()
            .map(|interface| quote_spanned!(span=> .bind(|__value| __value as ::std::sync::Arc<#interface>)));
        let placement = if let Some(name) = &provide.options.name {
            quote_spanned!(span=> .named(#name))
        } else {
            match &provide.options.group {
                Some(Some(group)) => quote_spanned!(span=> .in_group_named(#group)),
                Some(None) => quote_spanned!(span=> .in_group()),
                None => quote!(),
            }
        };
        quote_spanned!(span=> .provide(::fx::provide(<#self_ty>::#method) #(#hooks)* #bind #placement))
    });
    let invokes = invokes
        .iter()
        .map(|m| quote_spanned!(m.span()=> .invoke(<#self_ty>::#m)));
    let decorates = decorates
        .iter()
        .map(|m| quote_spanned!(m.span()=> .decorate(<#self_ty>::#m)));

    Ok(quote! {
        #item

        impl ::fx::Component for #self_ty {
            fn register(options: ::fx::Options) -> ::fx::Options {
                options #(#providers)* #(#invokes)* #(#decorates)*
            }
        }
    })
}
