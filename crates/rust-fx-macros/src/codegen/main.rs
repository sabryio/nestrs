use proc_macro2::TokenStream;
use quote::quote;

use crate::ir::Entry;
use crate::pipeline::Context;

/// Delegates to the runtime builder macro of the `tokio` that `fx` re-exports, pointing it at
/// that copy, so the program needs no direct `tokio` dependency just to have a `main`.
pub(crate) fn generate(ir: &Entry, cx: &Context) -> TokenStream {
    let (item, fx) = (&ir.item, &cx.fx);
    let options = &ir.options;
    let runtime = format!("{}::__private::tokio", quote!(#fx));
    quote! {
        #[#fx::__private::tokio::main(crate = #runtime #(, #options)*)]
        #item
    }
}
