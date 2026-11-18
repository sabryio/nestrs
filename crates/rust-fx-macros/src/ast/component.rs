use proc_macro2::{Span, TokenStream};
use syn::spanned::Spanned;
use syn::{FnArg, Ident, ImplItem, ItemImpl, Meta, ReceiverKind};

use super::PlacementOptions;
use crate::support::syntax::fx_name;

/// The attributes an `#[fx::component]` block recognises on its methods.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TagKind {
    Constructor,
    OnStart,
    OnStop,
    Invoke,
    Decorate,
}

impl TagKind {
    fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "constructor" => Self::Constructor,
            "on_start" => Self::OnStart,
            "on_stop" => Self::OnStop,
            "invoke" => Self::Invoke,
            "decorate" => Self::Decorate,
            _ => return None,
        })
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Constructor => "constructor",
            Self::OnStart => "on_start",
            Self::OnStop => "on_stop",
            Self::Invoke => "invoke",
            Self::Decorate => "decorate",
        }
    }
}

/// One `#[fx::..]` attribute found on a method.
pub(crate) struct Tag {
    pub(crate) kind: TagKind,
    pub(crate) span: Span,
    /// Only `#[fx::constructor(..)]` takes options.
    pub(crate) options: PlacementOptions,
}

/// How a method takes `self`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Receiver {
    /// Associated function: no `self`.
    None,
    /// `&self`.
    Borrowed,
    /// `self`, `&mut self`, `self: Arc<Self>`, ...
    Other,
}

/// A method carrying at least one of our attributes.
pub(crate) struct Member {
    pub(crate) ident: Ident,
    /// The span of the signature, for errors about the method as a whole.
    pub(crate) span: Span,
    pub(crate) receiver: Receiver,
    pub(crate) is_async: bool,
    pub(crate) tags: Vec<Tag>,
}

pub(crate) struct ComponentAst {
    /// The block with our attributes removed.
    pub(crate) item: ItemImpl,
    pub(crate) members: Vec<Member>,
}

pub(crate) fn parse(args: TokenStream, item: TokenStream) -> syn::Result<ComponentAst> {
    if !args.is_empty() {
        return Err(syn::Error::new_spanned(args, "#[fx::component] takes no arguments"));
    }
    let mut item: ItemImpl = syn::parse2(item)?;
    let mut members = Vec::new();

    for member in &mut item.items {
        let ImplItem::Fn(method) = member else { continue };

        let mut tags = Vec::new();
        let mut kept = Vec::new();
        for attr in std::mem::take(&mut method.attrs) {
            let kind = fx_name(attr.path()).and_then(|name| TagKind::from_name(&name));
            let Some(kind) = kind else {
                kept.push(attr);
                continue;
            };
            let options = match kind {
                TagKind::Constructor => PlacementOptions::from_meta(&attr.meta, "constructor")?,
                _ if matches!(attr.meta, Meta::Path(_)) => PlacementOptions::default(),
                _ => {
                    return Err(syn::Error::new_spanned(
                        &attr,
                        format!("`#[fx::{}]` takes no arguments", kind.name()),
                    ));
                }
            };
            tags.push(Tag {
                kind,
                span: attr.span(),
                options,
            });
        }
        method.attrs = kept;

        if tags.is_empty() {
            continue;
        }
        let receiver = match method.sig.inputs.first() {
            Some(FnArg::Receiver(receiver)) if matches!(&receiver.kind, ReceiverKind::Reference(_, _, None)) => {
                Receiver::Borrowed
            }
            Some(FnArg::Receiver(_)) => Receiver::Other,
            _ => Receiver::None,
        };
        members.push(Member {
            ident: method.sig.ident.clone(),
            span: method.sig.span(),
            receiver,
            is_async: method.sig.asyncness.is_some(),
            tags,
        });
    }
    Ok(ComponentAst { item, members })
}
