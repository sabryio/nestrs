//! Error accumulation for the analysis stage.
//!
//! A macro that stops at its first problem makes people fix errors one compile at a time.
//! [`Diagnostics`] collects every problem found and reports them together.

use std::fmt::Display;

use proc_macro2::{Span, TokenStream};
use quote::ToTokens;

/// A growing list of compile errors.
#[derive(Default)]
pub(crate) struct Diagnostics {
    error: Option<syn::Error>,
}

impl Diagnostics {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Reports `message` on the tokens it is about.
    pub(crate) fn error(&mut self, at: impl ToTokens, message: impl Display) {
        self.push(syn::Error::new_spanned(at, message));
    }

    /// Reports `message` at an explicit span.
    pub(crate) fn at(&mut self, span: Span, message: impl Display) {
        self.push(syn::Error::new(span, message));
    }

    pub(crate) fn push(&mut self, error: syn::Error) {
        match &mut self.error {
            Some(existing) => existing.combine(error),
            None => self.error = Some(error),
        }
    }

    /// The `compile_error!` invocations for everything reported, if anything was.
    pub(crate) fn into_tokens(self) -> Option<TokenStream> {
        self.error.map(|error| error.to_compile_error())
    }
}
