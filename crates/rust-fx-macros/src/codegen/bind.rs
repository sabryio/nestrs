use proc_macro2::TokenStream;
use quote::quote;

use crate::ir::{Binding, Placement};
use crate::pipeline::Context;

pub(crate) fn generate(ir: &Binding, cx: &Context) -> TokenStream {
    let fx = &cx.fx;
    let (item, component, interface) = (&ir.item, &ir.component, &ir.interface);
    let placement = match &ir.placement {
        Placement::Value => quote!(#fx::__private::Placement::Value),
        Placement::Named(name) => quote!(#fx::__private::Placement::Named(#name)),
        Placement::Group(None) => quote!(#fx::__private::Placement::Group),
        Placement::Group(Some(name)) => quote!(#fx::__private::Placement::GroupNamed(#name)),
    };
    quote! {
        #item

        impl #fx::Bound<dyn #interface> for #component {
            fn upcast(this: ::std::sync::Arc<Self>) -> ::std::sync::Arc<dyn #interface> {
                this
            }
        }

        #fx::__private::inventory::submit! {
            #fx::__private::Binding::new::<dyn #interface, #component>(#placement)
        }
    }
}
