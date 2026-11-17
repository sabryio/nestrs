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
    /// Contract generation failed.
    #[error(transparent)]
    Contract(#[from] crate::contract::ContractError),
    /// `doctor` found rule violations (already printed).
    #[error("{0} architecture problem(s) found")]
    Unhealthy(usize),
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

/// Where the project already gets nestrs from, read from its `nestrs = ..` line, so a new
/// dependency on a sibling crate follows the same source.
fn jobs_dependency(manifest: &str) -> String {
    for line in manifest.lines().map(str::trim) {
        let Some(rest) = line.strip_prefix("nestrs") else {
            continue;
        };
        let Some(spec) = rest.trim_start().strip_prefix('=') else {
            continue;
        };
        let spec = spec.trim();
        if let Some(from) = spec.find("path = \"") {
            let path = &spec[from + 8..];
            if let Some(end) = path.find('"') {
                let root = path[..end].trim_end_matches("/crates/nestrs");
                return format!("{{ path = \"{root}/crates/nestrs-jobs\" }}");
            }
        }
        if spec.starts_with('"') {
            return spec.to_owned();
        }
    }
    "\"0.1\"".to_owned()
}

/// `nestrs generate module <name>`: an empty module imported by `AppModule`.
pub fn new_module(workspace: &mut dyn Workspace, name: &str, options: Options) -> Result<Report, CliError> {
    let names = Names::parse(name, None)?;
    require_project(workspace)?;
    Ok(templates::module(&names).apply(workspace, options.force, options.dry_run)?)
}

/// `nestrs generate job <name>`: a scheduled job, wired into `AppModule` with `JobsModule`.
pub fn new_job(workspace: &mut dyn Workspace, name: &str, options: Options) -> Result<Report, CliError> {
    let names = Names::parse(name, None)?;
    let manifest = require_project(workspace)?;
    Ok(templates::job(&names, &jobs_dependency(&manifest)).apply(workspace, options.force, options.dry_run)?)
}

/// `nestrs generate queue-handler <name>`: a queue consumer, wired like a job.
pub fn new_queue_handler(workspace: &mut dyn Workspace, name: &str, options: Options) -> Result<Report, CliError> {
    let names = Names::parse(name, None)?;
    let manifest = require_project(workspace)?;
    Ok(templates::queue_handler(&names, &jobs_dependency(&manifest)).apply(
        workspace,
        options.force,
        options.dry_run,
    )?)
}

/// `nestrs generate use-case <feature> <action>`: a use case for an existing feature.
pub fn new_use_case(
    workspace: &mut dyn Workspace,
    feature: &str,
    plural: Option<&str>,
    action: &str,
    options: Options,
) -> Result<Report, CliError> {
    let feature = Names::parse(feature, plural)?;
    let action = Names::parse(action, None)?;
    require_project(workspace)?;
    Ok(templates::use_case(&feature, &action).apply(workspace, options.force, options.dry_run)?)
}

/// `nestrs generate controller <name>`: a controller, registered in `<module>_module.rs`.
pub fn new_controller(
    workspace: &mut dyn Workspace,
    name: &str,
    module: Option<&str>,
    options: Options,
) -> Result<Report, CliError> {
    let names = Names::parse(name, None)?;
    require_project(workspace)?;
    let module = module.unwrap_or(&names.snake).to_owned();
    Ok(templates::controller(&names, &module).apply(workspace, options.force, options.dry_run)?)
}

/// `nestrs generate port <name>`: a port, an in-memory adapter and the module that provides it.
pub fn new_port(workspace: &mut dyn Workspace, name: &str, options: Options) -> Result<Report, CliError> {
    let names = Names::parse(name, None)?;
    require_project(workspace)?;
    Ok(templates::port(&names).apply(workspace, options.force, options.dry_run)?)
}

fn require_project(workspace: &dyn Workspace) -> Result<String, CliError> {
    workspace
        .read(std::path::Path::new("Cargo.toml"))?
        .ok_or(CliError::NotAProject)
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
    fn a_job_adds_the_dependency_the_module_import_and_the_provider() {
        let mut ws = MemoryWorkspace::new();
        new_project(
            &mut ws,
            "shop",
            &DependencySource::Path("/src/nestrs".into()),
            Options::default(),
        )
        .unwrap();
        let report = new_job(&mut ws, "purge-drafts", Options::default()).unwrap();

        assert!(report.skipped.is_empty(), "{:?}", report.skipped);
        let cargo = ws.get("Cargo.toml").unwrap();
        assert!(
            cargo.contains("nestrs-jobs = { path = \"/src/nestrs/crates/nestrs-jobs\" }"),
            "{cargo}"
        );
        let app = ws.get("src/composition/app.rs").unwrap();
        assert!(app.contains("imports = [JobsModule::in_memory(), "), "{app}");
        assert!(app.contains("providers = [PurgeDraftsJob]"), "{app}");
        assert!(
            app.contains("use nestrs_jobs::JobsModule;") && app.contains("use crate::application::PurgeDraftsJob;")
        );

        // A second job shares JobsModule and extends the providers.
        new_queue_handler(&mut ws, "send-welcome", Options::default()).unwrap();
        let app = ws.get("src/composition/app.rs").unwrap();
        assert_eq!(app.matches("JobsModule::in_memory()").count(), 1, "{app}");
        assert!(
            app.contains("providers = [SendWelcomeHandler, PurgeDraftsJob]"),
            "{app}"
        );
        assert!(syn::parse_file(app).is_ok());
    }

    #[test]
    fn a_module_is_created_and_imported() {
        let mut ws = MemoryWorkspace::new();
        new_project(&mut ws, "shop", &source(), Options::default()).unwrap();
        new_module(&mut ws, "billing", Options::default()).unwrap();
        assert!(
            ws.get("src/composition/app.rs")
                .unwrap()
                .contains("imports = [BillingModule, ")
        );
        assert!(
            ws.get("src/composition/mod.rs")
                .unwrap()
                .contains("pub use billing_module::BillingModule;")
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
