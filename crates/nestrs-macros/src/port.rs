use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{Ident, ItemTrait, TypeParamBound, parse_quote};

/// `#[port]` or `#[port(CustomAlias)]` on a trait.
pub(crate) fn expand(args: TokenStream, mut item: ItemTrait) -> syn::Result<TokenStream> {
    if !item.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &item.generics,
            "#[port] does not support generic traits (the alias would need the same parameters)",
        ));
    }

    let trait_name = item.ident.clone();
    let alias: Ident = if args.is_empty() {
        format_ident!("{}Port", trait_name)
    } else {
        syn::parse2(args)?
    };

    // A port is shared across threads and stored in the container.
    for bound in [parse_quote!(Send), parse_quote!(Sync), parse_quote!('static)] {
        let bound: TypeParamBound = bound;
        item.supertraits.push(bound);
    }

    let vis = &item.vis;
    let doc = format!(
        "Shared handle to [`{trait_name}`]: declare it as a field type to inject the \
         implementation bound to this port."
    );
    Ok(quote! {
        #[::nestrs::async_trait]
        #item

        #[doc = #doc]
        #vis type #alias = ::std::sync::Arc<dyn #trait_name>;
    })
}
