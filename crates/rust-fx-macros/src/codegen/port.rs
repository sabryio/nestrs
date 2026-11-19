use proc_macro2::TokenStream;
use quote::quote;

use crate::ir::Port;

pub(crate) fn generate(ir: &Port) -> TokenStream {
    let (item, vis, alias, name) = (&ir.item, &ir.vis, &ir.alias, &ir.trait_ident);
    let doc =
        format!("The port `{name}` as a type: `Arc<{alias}>` to depend on it, `exports = [{alias}]` to publish it.");
    quote! {
        #item

        #[doc = #doc]
        #vis type #alias = dyn #name;
    }
}
