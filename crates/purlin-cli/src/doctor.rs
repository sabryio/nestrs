//! `purlin doctor`: does the project still follow its own architecture?
//!
//! The generated layout is hexagonal; the rules below are what keeps it that way. They are checked
//! by parsing, not by text search, so a comment mentioning `infrastructure` is not a violation.
//!
//! | Layer | May depend on | Must not touch |
//! |---|---|---|
//! | `domain` | itself | every other layer; web, database and queue crates |
//! | `application` | `domain` | `infrastructure`, `presentation`, `composition`; web, database and queue crates |
//! | `infrastructure` | `domain` (its ports) | `application`, `presentation`, `composition` |
//! | `presentation` | `application`, `domain` | `infrastructure`, `composition` |
//! | `composition` | anything (it wires the rest) | |
//!
//! It also reports controllers that no `#[module]` registers, which compiles and then 404s.

use std::collections::BTreeSet;
use std::path::Path;

use proc_macro2::{TokenStream, TokenTree};
use syn::{Item, UseTree};

use crate::workspace::Workspace;

/// The layers, innermost first.
const LAYERS: [&str; 5] = ["domain", "application", "infrastructure", "presentation", "composition"];

/// Crates that are adapters to the outside world, which inner layers must not name.
const ADAPTER_CRATES: &[&str] = &[
    "axum",
    "hyper",
    "tower",
    "tower_http",
    "sea_orm",
    "sea_query",
    "sqlx",
    "redis",
    "reqwest",
    "purlin_http",
    "purlin_sea_orm",
    "purlin_redis",
    "purlin_outbox",
];

/// Layers each layer must not reference through `crate::<layer>`.
fn forbidden_layers(layer: &str) -> &'static [&'static str] {
    match layer {
        "domain" => &["application", "infrastructure", "presentation", "composition"],
        "application" => &["infrastructure", "presentation", "composition"],
        "infrastructure" => &["application", "presentation", "composition"],
        "presentation" => &["infrastructure", "composition"],
        _ => &[],
    }
}

/// Layers that must also stay free of adapter crates.
fn is_inner(layer: &str) -> bool {
    matches!(layer, "domain" | "application")
}

/// One broken rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    /// The file.
    pub path: String,
    /// 1-based line.
    pub line: usize,
    /// What is wrong and what to do instead.
    pub message: String,
}

/// What the check found.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Diagnosis {
    /// Broken rules, ordered by file and line.
    pub violations: Vec<Violation>,
    /// Files that could not be parsed (reported, never ignored).
    pub unreadable: Vec<String>,
    /// Source files examined.
    pub files: usize,
}

impl Diagnosis {
    /// Whether the project passes.
    pub fn is_healthy(&self) -> bool {
        self.violations.is_empty() && self.unreadable.is_empty()
    }
}

/// The layer a project file belongs to (`src/domain/post.rs` → `domain`).
fn layer_of(path: &Path) -> Option<&'static str> {
    let mut parts = path.components().map(|c| c.as_os_str().to_string_lossy());
    if parts.next().as_deref() != Some("src") {
        return None;
    }
    let next = parts.next()?;
    let name = next.strip_suffix(".rs").unwrap_or(&next).to_owned();
    LAYERS.into_iter().find(|layer| *layer == name)
}

fn use_roots(tree: &UseTree, out: &mut Vec<(Vec<String>, usize)>, prefix: &mut Vec<String>) {
    match tree {
        UseTree::Path(path) => {
            prefix.push(path.ident.to_string());
            use_roots(&path.tree, out, prefix);
            prefix.pop();
        }
        UseTree::Name(name) => {
            let mut full = prefix.clone();
            full.push(name.ident.to_string());
            out.push((full, name.ident.span().start().line));
        }
        UseTree::Rename(rename) => {
            let mut full = prefix.clone();
            full.push(rename.ident.to_string());
            out.push((full, rename.ident.span().start().line));
        }
        UseTree::Glob(glob) => out.push((prefix.clone(), glob.star_token.spans[0].start().line)),
        UseTree::Group(group) => {
            for item in &group.items {
                use_roots(item, out, prefix);
            }
        }
    }
}

/// `(line, layer)` of every `crate::<layer>` reference in the tokens, wherever it appears.
fn crate_layer_refs(tokens: TokenStream, out: &mut Vec<(usize, String)>) {
    let tokens: Vec<TokenTree> = tokens.into_iter().collect();
    for (index, token) in tokens.iter().enumerate() {
        match token {
            TokenTree::Ident(ident) if ident == "crate" => {
                let colons = matches!(tokens.get(index + 1), Some(TokenTree::Punct(p)) if p.as_char() == ':')
                    && matches!(tokens.get(index + 2), Some(TokenTree::Punct(p)) if p.as_char() == ':');
                if colons && let Some(TokenTree::Ident(next)) = tokens.get(index + 3) {
                    out.push((ident.span().start().line, next.to_string()));
                }
            }
            TokenTree::Group(group) => crate_layer_refs(group.stream(), out),
            _ => {}
        }
    }
}

/// Idents inside every `controllers = [..]` (the attribute) and `.controllers([..])` (hand-written
/// modules), anywhere in the items.
fn registered_controllers(items: &[Item], out: &mut BTreeSet<String>) {
    use quote::ToTokens as _;
    for item in items {
        if let Item::Mod(module) = item
            && let Some((_, inner)) = &module.content
        {
            registered_controllers(inner, out);
        }
        let tokens: Vec<TokenTree> = item.to_token_stream().into_iter().collect();
        scan_controllers(&tokens, out);
    }
}

fn scan_controllers(tokens: &[TokenTree], out: &mut BTreeSet<String>) {
    for (index, token) in tokens.iter().enumerate() {
        match token {
            TokenTree::Ident(ident) if ident == "controllers" => {
                let after = match tokens.get(index + 1) {
                    Some(TokenTree::Punct(p)) if p.as_char() == '=' => tokens.get(index + 2),
                    other => other,
                };
                if let Some(TokenTree::Group(group)) = after {
                    let inner: Vec<TokenTree> = group.stream().into_iter().collect();
                    flatten_idents(&inner, out);
                }
            }
            TokenTree::Group(group) => {
                let inner: Vec<TokenTree> = group.stream().into_iter().collect();
                scan_controllers(&inner, out);
            }
            _ => {}
        }
    }
}

fn flatten_idents(tokens: &[TokenTree], out: &mut BTreeSet<String>) {
    for token in tokens {
        match token {
            TokenTree::Ident(ident) => {
                out.insert(ident.to_string());
            }
            TokenTree::Group(group) => {
                let inner: Vec<TokenTree> = group.stream().into_iter().collect();
                flatten_idents(&inner, out);
            }
            _ => {}
        }
    }
}

fn controllers_defined(items: &[Item], path: &str, out: &mut Vec<(String, String, usize)>) {
    use syn::spanned::Spanned as _;
    for item in items {
        match item {
            Item::Impl(item) if item.trait_.is_none() && item.attrs.iter().any(|a| a.path().is_ident("controller")) => {
                if let syn::Type::Path(ty) = &*item.self_ty
                    && let Some(segment) = ty.path.segments.last()
                {
                    out.push((segment.ident.to_string(), path.to_owned(), item.span().start().line));
                }
            }
            Item::Mod(item) => {
                if let Some((_, inner)) = &item.content {
                    controllers_defined(inner, path, out);
                }
            }
            _ => {}
        }
    }
}

/// Checks every file under `src/`.
pub fn diagnose(workspace: &dyn Workspace) -> std::io::Result<Diagnosis> {
    let mut diagnosis = Diagnosis::default();
    let mut defined = Vec::new();
    let mut registered = BTreeSet::new();

    for path in workspace.rust_files(Path::new("src"))? {
        let Some(text) = workspace.read(&path)? else { continue };
        diagnosis.files += 1;
        let shown = path.display().to_string();
        let Ok(file) = syn::parse_file(&text) else {
            diagnosis.unreadable.push(shown);
            continue;
        };
        registered_controllers(&file.items, &mut registered);
        controllers_defined(&file.items, &shown, &mut defined);

        let Some(layer) = layer_of(&path) else { continue };
        check_layer(layer, &shown, &file.items, &mut diagnosis.violations);
    }

    for (name, path, line) in defined {
        if !registered.contains(&name) {
            diagnosis.violations.push(Violation {
                path,
                line,
                message: format!("controller `{name}` is not listed in any #[module(controllers = [..])], so its routes are never mounted"),
            });
        }
    }
    diagnosis
        .violations
        .sort_by(|a, b| (&a.path, a.line, &a.message).cmp(&(&b.path, b.line, &b.message)));
    diagnosis.violations.dedup();
    Ok(diagnosis)
}

fn check_layer(layer: &str, path: &str, items: &[Item], out: &mut Vec<Violation>) {
    use quote::ToTokens as _;
    let forbidden = forbidden_layers(layer);

    // Every `crate::<layer>` mention, in `use` lines and in code alike.
    let mut refs = Vec::new();
    for item in items {
        crate_layer_refs(item.to_token_stream(), &mut refs);
    }
    for (line, target) in refs {
        if forbidden.contains(&target.as_str()) {
            out.push(Violation {
                path: path.to_owned(),
                line,
                message: format!("`{layer}` must not depend on `{target}`: {}", advice(layer, &target)),
            });
        }
    }

    if is_inner(layer) {
        for item in items {
            if let Item::Use(item) = item {
                let mut roots = Vec::new();
                use_roots(&item.tree, &mut roots, &mut Vec::new());
                for (full, line) in roots {
                    let root = full.first().map_or("", String::as_str);
                    if ADAPTER_CRATES.contains(&root) {
                        out.push(Violation {
                            path: path.to_owned(),
                            line,
                            message: format!(
                                "`{layer}` must not use the adapter crate `{root}`: define a port in `domain` and implement it in `infrastructure`"
                            ),
                        });
                    }
                }
            }
        }
    }
}

fn advice(layer: &str, target: &str) -> &'static str {
    match (layer, target) {
        ("presentation", "infrastructure") => {
            "depend on a use case in `application`, and let `composition` choose the adapter"
        }
        ("infrastructure", _) => "adapters implement domain ports and know nothing of who calls them",
        ("domain", _) => "the domain depends on nothing else in the project",
        ("application", _) => "use cases talk to ports defined in `domain`",
        _ => "move the dependency behind a port",
    }
}

/// Where a finding is, for display.
pub fn location(violation: &Violation) -> String {
    format!("{}:{}", violation.path, violation.line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::MemoryWorkspace;

    fn check(files: &[(&str, &str)]) -> Diagnosis {
        let mut ws = MemoryWorkspace::new();
        for (path, text) in files {
            ws = ws.with(*path, text);
        }
        diagnose(&ws).unwrap()
    }

    #[test]
    fn a_clean_project_passes() {
        let d = check(&[
            (
                "src/domain/post.rs",
                "use uuid::Uuid; use purlin::port; pub struct Post;",
            ),
            ("src/application/posts.rs", "use crate::domain::Post; pub struct P;"),
            (
                "src/presentation/http/posts.rs",
                "use crate::application::P; use axum::Json;",
            ),
            (
                "src/infrastructure/repo.rs",
                "use crate::domain::Post; use sea_orm::DbConn;",
            ),
            (
                "src/composition/app.rs",
                "use crate::infrastructure::Repo; use crate::presentation::C;",
            ),
        ]);
        assert!(d.is_healthy(), "{d:?}");
        assert_eq!(d.files, 5);
    }

    #[test]
    fn presentation_reaching_into_infrastructure_is_named_with_file_and_line() {
        let d = check(&[(
            "src/presentation/http/posts.rs",
            "use crate::application::P;\nuse crate::infrastructure::SeaRepo;\n",
        )]);
        assert_eq!(d.violations.len(), 1);
        let v = &d.violations[0];
        assert_eq!((v.path.as_str(), v.line), ("src/presentation/http/posts.rs", 2));
        assert!(
            v.message.contains("`presentation` must not depend on `infrastructure`"),
            "{}",
            v.message
        );
    }

    #[test]
    fn inline_paths_count_not_just_use_lines() {
        let d = check(&[(
            "src/application/x.rs",
            "fn f() { let _ = crate::infrastructure::Thing::new(); }",
        )]);
        assert_eq!(d.violations.len(), 1, "{d:?}");
    }

    #[test]
    fn the_domain_stays_free_of_other_layers_and_adapter_crates() {
        let d = check(&[(
            "src/domain/post.rs",
            "use crate::application::X;\nuse sea_orm::EntityTrait;\nuse axum::http::StatusCode;\n",
        )]);
        assert_eq!(d.violations.len(), 3, "{d:?}");
    }

    #[test]
    fn comments_and_strings_are_not_violations() {
        let d = check(&[(
            "src/domain/post.rs",
            "// uses crate::infrastructure\nconst S: &str = \"crate::infrastructure::X\";",
        )]);
        assert!(d.is_healthy(), "{d:?}");
    }

    #[test]
    fn composition_may_wire_anything() {
        let d = check(&[(
            "src/composition/m.rs",
            "use crate::infrastructure::A; use crate::presentation::B; use crate::domain::C;",
        )]);
        assert!(d.is_healthy());
    }

    #[test]
    fn an_unregistered_controller_is_reported() {
        let d = check(&[
            (
                "src/presentation/http/a.rs",
                "#[controller(\"/a\")]\nimpl AController { }",
            ),
            (
                "src/presentation/http/b.rs",
                "#[controller(\"/b\")]\nimpl BController { }",
            ),
            (
                "src/composition/m.rs",
                "#[module(controllers = [AController])]\npub struct M;",
            ),
        ]);
        assert_eq!(d.violations.len(), 1, "{d:?}");
        assert!(d.violations[0].message.contains("BController"));
    }

    #[test]
    fn unparseable_files_are_reported_not_skipped() {
        let d = check(&[("src/domain/bad.rs", "fn (")]);
        assert_eq!(d.unreadable, ["src/domain/bad.rs"]);
        assert!(!d.is_healthy());
    }
}
