use proc_macro2::TokenStream;
use syn::ItemImpl;

use super::PlacementOptions;

pub(crate) struct BindAst {
    pub(crate) item: ItemImpl,
    pub(crate) options: PlacementOptions,
}

pub(crate) fn parse(args: TokenStream, item: TokenStream) -> syn::Result<BindAst> {
    // The attribute's arguments are the *inside* of `#[fx::bind(..)]`, so parse them as the
    // meta list they would have been.
    let options = if args.is_empty() {
        PlacementOptions::default()
    } else {
        let meta: syn::Meta = syn::parse2(quote::quote!(bind(#args)))?;
        PlacementOptions::from_meta(&meta, "bind")?
    };
    Ok(BindAst {
        item: syn::parse2(item)?,
        options,
    })
}
