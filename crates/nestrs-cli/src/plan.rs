//! File operations as data, and how they are applied.

use std::io;
use std::path::PathBuf;

use crate::workspace::Workspace;

/// One change to the project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    /// A new file. Refused if the file exists, unless the run is forced.
    Create {
        /// Relative path.
        path: PathBuf,
        /// Whole contents.
        contents: String,
    },
    /// Makes sure each line is present in the file (created if missing). Idempotent.
    EnsureLines {
        /// Relative path.
        path: PathBuf,
        /// Lines to guarantee, appended in order when absent.
        lines: Vec<String>,
    },
    /// Edits Rust source found by parsing it (see [`patch`](crate::patch)). Skipped and
    /// reported, never guessed at, when the file cannot be patched.
    PatchRust {
        /// Relative path.
        path: PathBuf,
        /// What to change.
        patch: RustPatch,
    },
    /// Adds `name = spec` under `[dependencies]` in `Cargo.toml`, unless `name` is there already.
    AddDependency {
        /// Crate name.
        name: String,
        /// Right-hand side, such as `"0.1"` or `{ path = ".." }`.
        spec: String,
    },
    /// Inserts `text` right after the first `anchor`, unless `text` is already in the
    /// file. If the file or anchor is missing the op is *skipped* and reported, never
    /// guessed at.
    InsertAfter {
        /// Relative path.
        path: PathBuf,
        /// Text to find.
        anchor: String,
        /// Text to insert after it.
        text: String,
    },
}

/// A change to Rust source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RustPatch {
    /// Adds `item` to `key = [..]` of the file's `#[module(..)]`; `marker` detects it being there.
    ModuleEntry {
        /// `imports`, `providers`, `controllers`..
        key: String,
        /// What to add.
        item: String,
        /// The identifier that says it is already there.
        marker: String,
    },
    /// Adds `use <path>;` after the last `use`.
    Use {
        /// Path without `use` and `;`.
        path: String,
    },
}

/// An ordered list of [`Op`]s: what a generator wants done.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    /// In execution order.
    pub ops: Vec<Op>,
}

impl Plan {
    /// An empty plan.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a file to create.
    pub fn create(&mut self, path: impl Into<PathBuf>, contents: impl Into<String>) -> &mut Self {
        self.ops.push(Op::Create {
            path: path.into(),
            contents: contents.into(),
        });
        self
    }

    /// Adds lines to guarantee in a file.
    pub fn ensure_lines<I, S>(&mut self, path: impl Into<PathBuf>, lines: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.ops.push(Op::EnsureLines {
            path: path.into(),
            lines: lines.into_iter().map(Into::into).collect(),
        });
        self
    }

    /// Adds `module` to the `#[module]`'s `imports` in `path`, with the `use` that names it.
    pub fn import_module(&mut self, path: impl Into<PathBuf> + Clone, module: &str, use_path: &str) -> &mut Self {
        self.patch_module(path, "imports", module, module, Some(use_path))
    }

    /// Adds `item` to `key = [..]` of the `#[module]` in `path`, and `use <use_path>;` if given.
    pub fn patch_module(
        &mut self,
        path: impl Into<PathBuf> + Clone,
        key: &str,
        item: &str,
        marker: &str,
        use_path: Option<&str>,
    ) -> &mut Self {
        if let Some(use_path) = use_path {
            self.ops.push(Op::PatchRust {
                path: path.clone().into(),
                patch: RustPatch::Use {
                    path: use_path.to_owned(),
                },
            });
        }
        self.ops.push(Op::PatchRust {
            path: path.into(),
            patch: RustPatch::ModuleEntry {
                key: key.to_owned(),
                item: item.to_owned(),
                marker: marker.to_owned(),
            },
        });
        self
    }

    /// Adds a dependency to `Cargo.toml`.
    pub fn add_dependency(&mut self, name: &str, spec: &str) -> &mut Self {
        self.ops.push(Op::AddDependency {
            name: name.to_owned(),
            spec: spec.to_owned(),
        });
        self
    }

    /// Adds an insertion after an anchor.
    pub fn insert_after(
        &mut self,
        path: impl Into<PathBuf>,
        anchor: impl Into<String>,
        text: impl Into<String>,
    ) -> &mut Self {
        self.ops.push(Op::InsertAfter {
            path: path.into(),
            anchor: anchor.into(),
            text: text.into(),
        });
        self
    }

    /// Applies the plan. With `dry_run` nothing is written, but the report is the same.
    ///
    /// Existing files are never overwritten unless `force`; a skipped op is reported, it
    /// does not abort the others.
    pub fn apply(&self, workspace: &mut dyn Workspace, force: bool, dry_run: bool) -> io::Result<Report> {
        let mut report = Report::default();
        for op in &self.ops {
            match op {
                Op::Create { path, contents } => {
                    let exists = workspace.read(path)?.is_some();
                    if exists && !force {
                        report
                            .skipped
                            .push(format!("{} (exists; use --force to overwrite)", path.display()));
                        continue;
                    }
                    if !dry_run {
                        workspace.write(path, contents)?;
                    }
                    report.created.push(path.display().to_string());
                }
                Op::EnsureLines { path, lines } => {
                    let existing = workspace.read(path)?;
                    let mut text = existing.clone().unwrap_or_default();
                    let mut changed = existing.is_none();
                    for line in lines {
                        if !text.lines().any(|present| present.trim() == line.trim()) {
                            if !text.is_empty() && !text.ends_with('\n') {
                                text.push('\n');
                            }
                            text.push_str(line);
                            text.push('\n');
                            changed = true;
                        }
                    }
                    if changed {
                        if !dry_run {
                            workspace.write(path, &text)?;
                        }
                        report.note_updated(path);
                    }
                }
                Op::PatchRust { path, patch } => {
                    let Some(existing) = workspace.read(path)? else {
                        report
                            .skipped
                            .push(format!("{} (missing: apply {patch:?} yourself)", path.display()));
                        continue;
                    };
                    let outcome = match patch {
                        RustPatch::ModuleEntry { key, item, marker } => {
                            crate::patch::add_module_entry(&existing, key, item, marker)
                        }
                        RustPatch::Use { path } => crate::patch::add_use(&existing, path),
                    };
                    match outcome {
                        Ok(None) => {}
                        Ok(Some(patched)) => {
                            if !dry_run {
                                workspace.write(path, &patched)?;
                            }
                            report.note_updated(path);
                        }
                        Err(error) => report
                            .skipped
                            .push(format!("{} ({error}: apply {patch:?} yourself)", path.display())),
                    }
                }
                Op::AddDependency { name, spec } => {
                    let manifest = std::path::Path::new("Cargo.toml");
                    let Some(existing) = workspace.read(manifest)? else {
                        report
                            .skipped
                            .push(format!("Cargo.toml (missing: add `{name} = {spec}` yourself)"));
                        continue;
                    };
                    let declared = existing.lines().any(|line| {
                        line.trim_start()
                            .strip_prefix(name.as_str())
                            .is_some_and(|rest| rest.trim_start().starts_with('='))
                    });
                    if declared {
                        continue;
                    }
                    let Some(at) = existing.find("[dependencies]") else {
                        report.skipped.push(format!(
                            "Cargo.toml (no [dependencies]: add `{name} = {spec}` yourself)"
                        ));
                        continue;
                    };
                    let line_end = existing[at..].find('\n').map_or(existing.len(), |n| at + n + 1);
                    let mut patched = existing.clone();
                    patched.insert_str(line_end, &format!("{name} = {spec}\n"));
                    if !dry_run {
                        workspace.write(manifest, &patched)?;
                    }
                    report.note_updated(manifest);
                }
                Op::InsertAfter { path, anchor, text } => {
                    let Some(existing) = workspace.read(path)? else {
                        report
                            .skipped
                            .push(format!("{} (missing: add `{}` yourself)", path.display(), text.trim()));
                        continue;
                    };
                    if existing.contains(text.as_str()) {
                        continue;
                    }
                    let Some(at) = existing.find(anchor.as_str()) else {
                        report.skipped.push(format!(
                            "{} (no `{}`: add `{}` yourself)",
                            path.display(),
                            anchor,
                            text.trim()
                        ));
                        continue;
                    };
                    let end = at + anchor.len();
                    let patched = format!("{}{}{}", &existing[..end], text, &existing[end..]);
                    if !dry_run {
                        workspace.write(path, &patched)?;
                    }
                    report.note_updated(path);
                }
            }
        }
        if !dry_run {
            let created: Vec<PathBuf> = report.created.iter().map(PathBuf::from).collect();
            workspace.format(&created);
        }
        Ok(report)
    }
}

/// What a run did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    /// Files written for the first time.
    pub created: Vec<String>,
    /// Existing files that were changed.
    pub updated: Vec<String>,
    /// Things left alone, each with the reason and what to do by hand.
    pub skipped: Vec<String>,
}

impl Report {
    /// Records a changed file once, however many ops touched it.
    fn note_updated(&mut self, path: &std::path::Path) {
        let path = path.display().to_string();
        if !self.updated.contains(&path) {
            self.updated.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::MemoryWorkspace;

    #[test]
    fn create_refuses_to_overwrite_unless_forced() {
        let mut ws = MemoryWorkspace::new().with("a.rs", "old");
        let mut plan = Plan::new();
        plan.create("a.rs", "new");
        let report = plan.apply(&mut ws, false, false).unwrap();
        assert_eq!(ws.get("a.rs"), Some("old"));
        assert_eq!(report.skipped.len(), 1);

        plan.apply(&mut ws, true, false).unwrap();
        assert_eq!(ws.get("a.rs"), Some("new"));
    }

    #[test]
    fn ensure_lines_is_idempotent() {
        let mut ws = MemoryWorkspace::new().with("mod.rs", "mod a;\n");
        let mut plan = Plan::new();
        plan.ensure_lines("mod.rs", ["mod a;", "mod b;"]);
        plan.apply(&mut ws, false, false).unwrap();
        let second = plan.apply(&mut ws, false, false).unwrap();
        assert_eq!(ws.get("mod.rs"), Some("mod a;\nmod b;\n"));
        assert!(second.updated.is_empty(), "nothing left to change");
    }

    #[test]
    fn insert_after_patches_once_and_reports_what_it_cannot_do() {
        let mut ws = MemoryWorkspace::new().with("app.rs", "imports = [Kit]");
        let mut plan = Plan::new();
        plan.insert_after("app.rs", "imports = [", "Billing, ");
        plan.apply(&mut ws, false, false).unwrap();
        plan.apply(&mut ws, false, false).unwrap();
        assert_eq!(ws.get("app.rs"), Some("imports = [Billing, Kit]"));

        let mut missing = Plan::new();
        missing.insert_after("app.rs", "nope", "X");
        missing.insert_after("other.rs", "nope", "X");
        let report = missing.apply(&mut ws, false, false).unwrap();
        assert_eq!(report.skipped.len(), 2);
        assert_eq!(ws.get("app.rs"), Some("imports = [Billing, Kit]"));
    }

    #[test]
    fn a_dry_run_reports_but_writes_nothing() {
        let mut ws = MemoryWorkspace::new();
        let mut plan = Plan::new();
        plan.create("a.rs", "x");
        let report = plan.apply(&mut ws, false, true).unwrap();
        assert_eq!(report.created, ["a.rs"]);
        assert!(ws.paths().is_empty());
    }
}
