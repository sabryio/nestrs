use proc_macro2::TokenStream;
use quote::quote;
use syn::{Attribute, FnArg, ImplItem, ImplItemFn, ItemImpl, Result, ReturnType};

/// `#[lifecycle]` on an inherent impl: `#[on_init]` / `#[on_destroy]` methods become
/// the type's `OnModuleInit` / `OnModuleDestroy` implementations.
pub(crate) fn expand(mut item: ItemImpl) -> Result<TokenStream> {
    if item.trait_.is_some() {
        return Err(syn::Error::new_spanned(
            &item,
            "#[lifecycle] goes on an inherent `impl Type { .. }` block, not a trait impl",
        ));
    }

    let mut init = None;
    let mut destroy = None;
    for member in &mut item.items {
        let ImplItem::Fn(method) = member else { continue };
        let on_init = take(&mut method.attrs, "on_init");
        let on_destroy = take(&mut method.attrs, "on_destroy");
        if on_init && on_destroy {
            return Err(syn::Error::new_spanned(
                &method.sig,
                "a method cannot be both #[on_init] and #[on_destroy]",
            ));
        }
        if on_init || on_destroy {
            check_signature(method)?;
            let slot = if on_init { &mut init } else { &mut destroy };
            if slot.is_some() {
                let which = if on_init { "#[on_init]" } else { "#[on_destroy]" };
                return Err(syn::Error::new_spanned(
                    &method.sig,
                    format!("only one {which} method is allowed"),
                ));
            }
            *slot = Some(method.sig.ident.clone());
        }
    }
    if init.is_none() && destroy.is_none() {
        return Err(syn::Error::new_spanned(
            &item.self_ty,
            "#[lifecycle] needs at least one `#[on_init]` or `#[on_destroy]` method",
        ));
    }

    let ty = &item.self_ty;
    let (impl_generics, _, where_clause) = item.generics.split_for_impl();
    let init_impl = init.map(|method| {
        quote! {
            impl #impl_generics ::purlin::OnModuleInit for #ty #where_clause {
                fn on_module_init(&self)
                    -> impl ::std::future::Future<Output = ::std::result::Result<(), ::purlin::DiError>> + ::std::marker::Send
                {
                    async move { #ty::#method(self).await.map_err(::purlin::DiError::custom) }
                }
            }
        }
    });
    let destroy_impl = destroy.map(|method| {
        quote! {
            impl #impl_generics ::purlin::OnModuleDestroy for #ty #where_clause {
                fn on_module_destroy(&self)
                    -> impl ::std::future::Future<Output = ::std::result::Result<(), ::purlin::DiError>> + ::std::marker::Send
                {
                    async move { #ty::#method(self).await.map_err(::purlin::DiError::custom) }
                }
            }
        }
    });

    Ok(quote! {
        #item
        #init_impl
        #destroy_impl
    })
}

fn take(attrs: &mut Vec<Attribute>, name: &str) -> bool {
    let before = attrs.len();
    attrs.retain(|attr| !attr.path().is_ident(name));
    attrs.len() != before
}

fn check_signature(method: &ImplItemFn) -> Result<()> {
    let sig = &method.sig;
    if sig.asyncness.is_none() {
        return Err(syn::Error::new_spanned(sig, "lifecycle methods must be `async fn`"));
    }
    let takes_only_self = sig.inputs.len() == 1
        && matches!(sig.inputs.first(), Some(FnArg::Receiver(r)) if crate::util::is_shared_self(r));
    if !takes_only_self {
        return Err(syn::Error::new_spanned(sig, "lifecycle methods take only `&self`"));
    }
    if matches!(sig.output, ReturnType::Default) {
        return Err(syn::Error::new_spanned(
            sig,
            "lifecycle methods must return `Result<(), E>` where `E: Display`",
        ));
    }
    Ok(())
}
