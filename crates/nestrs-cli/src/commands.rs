//! The two commands, as functions over a [`Workspace`].

use std::io;

use crate::naming::{NameError, Names, crate_names};
use crate::plan::Report;
use crate::templates::{self, DependencySource};
use crate::workspace::Workspace;

/// A command failed.
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    /// The name was refused.
    #[error(transparent)]
    Name(#[from] NameError),
    /// A file could not be read or written.
    #[error("file system error: {0}")]
    Io(#[from] io::Error),
    /// `generate` was run outside a nestrs project.
    #[error("no Cargo.toml here: run this inside a project created by `nestrs new`")]
    NotAProject,
    /// The project's library name could not be found.
    #[error("cannot tell the library name: Cargo.toml has no `name = \"..\"`")]
    NoPackageName,
}

/// Options shared by every command.
#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    /// Overwrite files that already exist.
    pub force: bool,
    /// Report what would happen without writing.
    pub dry_run: bool,
}

/// `nestrs new <name>`: writes the project into `workspace` (whose root is the new directory).
pub fn new_project(
    workspace: &mut dyn Workspace,
    name: &str,
    source: &DependencySource,
    options: Options,
) -> Result<Report, CliError> {
    let (package, lib) = crate_names(name)?;
    Ok(templates::project(&package, &lib, source).apply(workspace, options.force, options.dry_run)?)
}

/// `nestrs generate feature <name>`: adds a vertical slice to the project in `workspace`.
pub fn new_feature(
    workspace: &mut dyn Workspace,
    name: &str,
    plural: Option<&str>,
    options: Options,
) -> Result<Report, CliError> {
    let names = Names::parse(name, plural)?;
    let manifest = workspace
        .read(std::path::Path::new("Cargo.toml"))?
        .ok_or(CliError::NotAProject)?;
    let lib = package_name(&manifest)
        .ok_or(CliError::NoPackageName)?
        .replace('-', "_");
    Ok(templates::feature(&names, &lib).apply(workspace, options.force, options.dry_run)?)
}

/// The `name` in the `[package]` table.
fn package_name(manifest: &str) -> Option<String> {
    let mut in_package = false;
    for line in manifest.lines().map(str::trim) {
        if line.starts_with('[') {
            in_package = line == "[package]";
        } else if in_package && let Some(rest) = line.strip_prefix("name") {
            let value = rest.trim_start().strip_prefix('=')?.trim();
            return Some(value.trim_matches('"').to_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::MemoryWorkspace;

    fn source() -> DependencySource {
        DependencySource::Version("0.1".into())
    }

    #[test]
    fn a_new_project_then_a_feature_wire_themselves_together() {
        let mut ws = MemoryWorkspace::new();
        new_project(&mut ws, "shop-api", &source(), Options::default()).unwrap();
        let report = new_feature(&mut ws, "invoice", None, Options::default()).unwrap();

        assert!(report.skipped.is_empty(), "{:?}", report.skipped);
        let app = ws.get("src/composition/app.rs").unwrap();
        assert!(app.contains("use super::InvoicesModule;"), "{app}");
        assert!(
            app.contains("imports = [InvoicesModule, HttpModule::module(), KitModule]"),
            "{app}"
        );
        assert!(ws.get("src/domain/mod.rs").unwrap().contains("mod invoice;"));
        assert!(
            ws.get("tests/invoices.rs")
                .unwrap()
                .contains("use shop_api::AppModule;")
        );
    }

    #[test]
    fn running_a_feature_twice_changes_nothing_the_second_time() {
        let mut ws = MemoryWorkspace::new();
        new_project(&mut ws, "shop", &source(), Options::default()).unwrap();
        new_feature(&mut ws, "invoice", None, Options::default()).unwrap();
        let before = ws.clone();
        let again = new_feature(&mut ws, "invoice", None, Options::default()).unwrap();
        assert!(again.created.is_empty() && again.updated.is_empty(), "{again:?}");
        assert_eq!(before.paths(), ws.paths());
        assert_eq!(before.get("src/composition/app.rs"), ws.get("src/composition/app.rs"));
    }

    #[test]
    fn generating_outside_a_project_is_a_clear_error() {
        let mut ws = MemoryWorkspace::new();
        assert!(matches!(
            new_feature(&mut ws, "invoice", None, Options::default()),
            Err(CliError::NotAProject)
        ));
    }

    #[test]
    fn a_bad_name_writes_nothing() {
        let mut ws = MemoryWorkspace::new();
        assert!(matches!(
            new_project(&mut ws, "9bad", &source(), Options::default()),
            Err(CliError::Name(_))
        ));
        assert!(ws.paths().is_empty());
    }
}
