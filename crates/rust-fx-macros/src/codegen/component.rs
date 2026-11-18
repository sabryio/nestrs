use proc_macro2::{Span, TokenStream};
use quote::{quote, quote_spanned};
use syn::Ident;

use crate::ir::{Component, Hook, Phase, Placement};
use crate::pipeline::Context;

/// The builder call that places a constructor's value.
fn placement(placement: &Placement, span: Span) -> TokenStream {
    match placement {
        Placement::Value => quote!(),
        Placement::Named(name) => quote_spanned!(span=> .named(#name)),
        Placement::Group(Some(group)) => quote_spanned!(span=> .in_group_named(#group)),
        Placement::Group(None) => quote_spanned!(span=> .in_group()),
    }
}

fn hook(hook: &Hook, self_ty: &syn::Type) -> TokenStream {
    let method = &hook.method;
    let register = Ident::new(
        match hook.phase {
            Phase::Start => "on_start",
            Phase::Stop => "on_stop",
        },
        Span::call_site(),
    );
    let body = if hook.is_async {
        quote!(async move { __this.#method().await })
    } else {
        quote!(async move { __this.#method() })
    };
    quote!(.#register(|__this: ::std::sync::Arc<#self_ty>| #body))
}

pub(crate) fn generate(ir: &Component, cx: &Context) -> TokenStream {
    let fx = &cx.fx;
    let item = &ir.item;
    let self_ty = &*item.self_ty;

    // Start hooks first, then stop hooks, in declaration order within each phase.
    let hooks: Vec<_> = [Phase::Start, Phase::Stop]
        .into_iter()
        .flat_map(|phase| ir.hooks.iter().filter(move |h| h.phase == phase))
        .map(|h| hook(h, self_ty))
        .collect();

    let constructors = ir.constructors.iter().map(|constructor| {
        let method = &constructor.method;
        let span = method.span();
        let placed = placement(&constructor.placement, span);
        quote_spanned!(span=> .constructor(#fx::constructor(<#self_ty>::#method) #(#hooks)* #placed))
    });
    let invokes = ir
        .invokes
        .iter()
        .map(|m| quote_spanned!(m.span()=> .invoke(<#self_ty>::#m)));
    let decorators = ir
        .decorators
        .iter()
        .map(|m| quote_spanned!(m.span()=> .decorate(<#self_ty>::#m)));

    quote! {
        #item

        impl #fx::Component for #self_ty {
            fn register(options: #fx::Options) -> #fx::Options {
                options #(#constructors)* #(#invokes)* #(#decorators)*
            }
        }
    }
}
