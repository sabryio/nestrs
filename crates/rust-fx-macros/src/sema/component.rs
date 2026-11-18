use proc_macro2::Span;

use crate::ast::component::{ComponentAst, Member, Receiver, TagKind};
use crate::diagnostics::Diagnostics;
use crate::ir::{self, Constructor, Hook, Phase};

pub(crate) fn analyze(ast: &ComponentAst, diagnostics: &mut Diagnostics) -> ir::Component {
    let item = &ast.item;
    if let Some((path, _)) = &item.trait_ {
        diagnostics.error(
            path,
            "#[fx::component] goes on an inherent `impl Type { .. }` block, not a trait impl \
             (use #[fx::bind] to declare an interface)",
        );
    }
    if !item.generics.params.is_empty() {
        diagnostics.error(
            &item.generics,
            "#[fx::component] does not support generic impls; implement `fx::Component` by hand",
        );
    }

    let mut component = ir::Component {
        item: item.clone(),
        constructors: Vec::new(),
        hooks: Vec::new(),
        invokes: Vec::new(),
        decorators: Vec::new(),
    };

    for member in &ast.members {
        for tag in &member.tags {
            match tag.kind {
                TagKind::Constructor => {
                    require_associated(member, tag.kind, tag.span, diagnostics);
                    let placement = super::placement(&tag.options, &member.ident, "constructor", diagnostics);
                    component.constructors.push(Constructor {
                        method: member.ident.clone(),
                        placement,
                    });
                }
                TagKind::OnStart | TagKind::OnStop => {
                    if member.receiver != Receiver::Borrowed {
                        diagnostics.at(
                            member.span,
                            format!(
                                "`#[fx::{}]` methods must take `&self` (the built value is shared)",
                                tag.kind.name()
                            ),
                        );
                    }
                    component.hooks.push(Hook {
                        method: member.ident.clone(),
                        phase: if tag.kind == TagKind::OnStart {
                            Phase::Start
                        } else {
                            Phase::Stop
                        },
                        is_async: member.is_async,
                    });
                }
                TagKind::Invoke => {
                    require_associated(member, tag.kind, tag.span, diagnostics);
                    component.invokes.push(member.ident.clone());
                }
                TagKind::Decorate => {
                    require_associated(member, tag.kind, tag.span, diagnostics);
                    component.decorators.push(member.ident.clone());
                }
            }
        }
    }

    if component.constructors.is_empty() && !component.hooks.is_empty() {
        diagnostics.at(
            Span::call_site(),
            "`#[fx::on_start]` / `#[fx::on_stop]` need a `#[fx::constructor]` in the same impl block",
        );
    }
    component
}

fn require_associated(member: &Member, kind: TagKind, _at: Span, diagnostics: &mut Diagnostics) {
    if member.receiver != Receiver::None {
        diagnostics.at(
            member.span,
            format!(
                "`#[fx::{}]` functions take their dependencies as parameters, not `self`",
                kind.name()
            ),
        );
    }
}
