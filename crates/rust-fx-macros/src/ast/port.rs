use proc_macro2::TokenStream;
use syn::{Ident, ItemTrait};

pub(crate) struct PortAst {
    pub(crate) item: ItemTrait,
    /// `#[fx::port(Name)]`: the alias to generate instead of `<Trait>Port`.
    pub(crate) alias: Option<Ident>,
}

pub(crate) fn parse(args: TokenStream, item: TokenStream) -> syn::Result<PortAst> {
    let alias = if args.is_empty() {
        None
    } else {
        Some(syn::parse2::<Ident>(args)?)
    };
    Ok(PortAst {
        item: syn::parse2(item)?,
        alias,
    })
}
