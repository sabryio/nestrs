use proc_macro2::TokenStream;
use syn::punctuated::Punctuated;
use syn::{ItemFn, Meta, Token};

/// `#[fx::main(..)]`: the function, and the options to hand to the runtime builder.
pub(crate) struct MainAst {
    pub(crate) item: ItemFn,
    /// `flavor = "current_thread"`, `worker_threads = 2`, ... (same as `#[tokio::main(..)]`).
    pub(crate) options: Punctuated<Meta, Token![,]>,
}

pub(crate) fn parse(args: TokenStream, item: TokenStream) -> syn::Result<MainAst> {
    let options = syn::parse::Parser::parse2(Punctuated::<Meta, Token![,]>::parse_terminated, args)?;
    Ok(MainAst {
        item: syn::parse2(item)?,
        options,
    })
}
