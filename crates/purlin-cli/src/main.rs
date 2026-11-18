//! The `purlin` command line.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use purlin_cli::commands::Options;
use purlin_cli::templates::DependencySource;
use purlin_cli::{DiskWorkspace, Report, commands};

#[derive(Parser)]
#[command(name = "purlin", version, about = "Scaffold purlin projects and features")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Args, Clone, Copy)]
struct Common {
    /// Overwrite files that already exist.
    #[arg(long)]
    force: bool,
    /// Show what would change as a diff, without writing.
    #[arg(long)]
    dry_run: bool,
}

impl From<Common> for Options {
    fn from(common: Common) -> Self {
        Self {
            force: common.force,
            dry_run: common.dry_run,
        }
    }
}

#[derive(Subcommand)]
enum Command {
    /// Create a new clean-architecture service in `./<name>`.
    New {
        /// Crate name, e.g. `shop-api`.
        name: String,
        /// Use a local checkout of purlin instead of the published crates.
        #[arg(long, value_name = "DIR")]
        purlin_path: Option<String>,
        #[command(flatten)]
        common: Common,
    },
    /// Check the project against its architecture: layer dependencies (domain and application
    /// stay free of infrastructure and web/DB crates; presentation never reaches infrastructure)
    /// and controllers that no module registers. Exits non-zero on findings, so CI can gate on it.
    Doctor,
    /// Generate code inside the current project.
    #[command(alias = "g")]
    Generate {
        #[command(subcommand)]
        what: Generate,
    },
}

#[derive(Subcommand)]
enum Generate {
    /// A vertical slice: domain entity + port, use cases, in-memory adapter, controller, module and test.
    Feature {
        /// Singular name, e.g. `invoice` or `invoice-line`.
        name: String,
        /// Plural, when it is irregular (`person` → `people`).
        #[arg(long)]
        plural: Option<String>,
        #[command(flatten)]
        common: Common,
    },
    /// An empty feature module in `composition/`, imported by `AppModule`.
    Module {
        /// Singular name, e.g. `billing`.
        name: String,
        #[command(flatten)]
        common: Common,
    },
    /// A use case for an existing feature (`use-case invoice archive` → `ArchiveInvoice`), with a
    /// unit test, registered in the feature's module.
    UseCase {
        /// The feature it belongs to, e.g. `invoice`.
        feature: String,
        /// The verb, e.g. `archive`.
        action: String,
        /// The feature's plural, when irregular.
        #[arg(long)]
        plural: Option<String>,
        #[command(flatten)]
        common: Common,
    },
    /// A controller in `presentation/http`, registered in a module.
    Controller {
        /// Singular name, e.g. `billing` (route `/billings`).
        name: String,
        /// File stem of the module to register it in (`invoices` → `composition/invoices_module.rs`).
        /// Default: the controller's own name.
        #[arg(long, value_name = "STEM")]
        module: Option<String>,
        #[command(flatten)]
        common: Common,
    },
    /// A port in `domain`, an in-memory adapter in `infrastructure`, and the module providing it.
    Port {
        /// Name of the port, e.g. `mailer` or `payment-gateway`.
        name: String,
        #[command(flatten)]
        common: Common,
    },
    /// The TypeScript oRPC contract, derived from the Rust controllers and DTOs (read with `syn`;
    /// no build, no running server). Re-run it whenever the API changes.
    Contract {
        /// Where to write the contract, relative to the project root.
        #[arg(long, value_name = "FILE", default_value = "contract.generated.ts")]
        out: PathBuf,
        /// Another source directory to read controllers and DTOs from (repeatable), e.g. a
        /// library crate that ships its own controller.
        #[arg(long, value_name = "DIR")]
        also: Vec<PathBuf>,
        /// Where the server mounts the API. Default: the `.prefix("..")` in `src/main.rs`.
        #[arg(long)]
        prefix: Option<String>,
        /// Do not write; exit non-zero (showing the diff) when the file is out of date. For CI.
        #[arg(long)]
        check: bool,
        /// Exit non-zero when anything had to be left out of the contract.
        #[arg(long)]
        strict: bool,
        /// Show the diff without writing.
        #[arg(long)]
        dry_run: bool,
    },
    /// A scheduled job (`purlin-jobs`), registered in `AppModule` with its `JobsModule`.
    Job {
        /// Name, e.g. `purge-drafts`.
        name: String,
        #[command(flatten)]
        common: Common,
    },
    /// A queue consumer and its message type (`purlin-jobs`), registered like a job.
    QueueHandler {
        /// Name, e.g. `send-welcome`; also the queue's name.
        name: String,
        #[command(flatten)]
        common: Common,
    },
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), commands::CliError> {
    match cli.command {
        Command::Doctor => {
            let workspace = DiskWorkspace::new(".");
            let diagnosis = purlin_cli::doctor::diagnose(&workspace)?;
            for path in &diagnosis.unreadable {
                eprintln!("{path}: cannot be parsed as Rust");
            }
            for violation in &diagnosis.violations {
                eprintln!("{}: {}", purlin_cli::doctor::location(violation), violation.message);
            }
            let problems = diagnosis.violations.len() + diagnosis.unreadable.len();
            if problems > 0 {
                return Err(commands::CliError::Unhealthy(problems));
            }
            println!("healthy: {} files follow the layer rules", diagnosis.files);
        }
        Command::New {
            name,
            purlin_path,
            common,
        } => {
            let source = purlin_path.map_or_else(
                || {
                    DependencySource::Version(
                        env!("CARGO_PKG_VERSION")
                            .rsplit_once('.')
                            .map_or("0.1", |(minor, _)| minor)
                            .to_owned(),
                    )
                },
                DependencySource::Path,
            );
            let mut workspace = DiskWorkspace::new(PathBuf::from(&name));
            let report = commands::new_project(&mut workspace, &name, &source, common.into())?;
            print(&report, common.dry_run);
            println!("\nNext:\n  cd {name}\n  purlin generate feature <name>\n  cargo run");
        }
        Command::Generate {
            what:
                Generate::Contract {
                    out,
                    also,
                    prefix,
                    check,
                    strict,
                    dry_run,
                },
        } => {
            let mut workspace = DiskWorkspace::new(".");
            let options = purlin_cli::contract::ContractOptions {
                out,
                also,
                prefix,
                check,
                strict,
                dry_run,
            };
            let report = match purlin_cli::contract::run(&mut workspace, &options) {
                Err(commands::CliError::Contract(purlin_cli::contract::ContractError::Stale { path, diff })) => {
                    print!("{diff}");
                    return Err(commands::CliError::Contract(
                        purlin_cli::contract::ContractError::Stale {
                            path,
                            diff: String::new(),
                        },
                    ));
                }
                other => other?,
            };
            for warning in &report.warnings {
                eprintln!("warning: {warning}");
            }
            if dry_run {
                print!("{}", report.diff);
            }
            let verb = match (check, dry_run, report.changed) {
                (true, _, _) => "up to date",
                (_, true, true) => "would write",
                (_, _, true) => "wrote",
                _ => "unchanged",
            };
            println!(
                "{verb:<10} {} ({} routes, {} schemas)",
                report.path, report.routes, report.schemas
            );
        }
        Command::Generate { what } => {
            let mut workspace = DiskWorkspace::new(".");
            let (report, common) = match what {
                Generate::Feature { name, plural, common } => (
                    commands::new_feature(&mut workspace, &name, plural.as_deref(), common.into())?,
                    common,
                ),
                Generate::Module { name, common } => {
                    (commands::new_module(&mut workspace, &name, common.into())?, common)
                }
                Generate::Job { name, common } => (commands::new_job(&mut workspace, &name, common.into())?, common),
                Generate::Contract { .. } => unreachable!("handled above"),
                Generate::UseCase {
                    feature,
                    action,
                    plural,
                    common,
                } => (
                    commands::new_use_case(&mut workspace, &feature, plural.as_deref(), &action, common.into())?,
                    common,
                ),
                Generate::Controller { name, module, common } => (
                    commands::new_controller(&mut workspace, &name, module.as_deref(), common.into())?,
                    common,
                ),
                Generate::Port { name, common } => (commands::new_port(&mut workspace, &name, common.into())?, common),
                Generate::QueueHandler { name, common } => (
                    commands::new_queue_handler(&mut workspace, &name, common.into())?,
                    common,
                ),
            };
            print(&report, common.dry_run);
        }
    }
    Ok(())
}

fn print(report: &Report, dry_run: bool) {
    let verb = |past: &'static str, dry: &'static str| if dry_run { dry } else { past };
    for path in &report.created {
        println!("{:<8} {path}", verb("create", "would create"));
    }
    for path in &report.updated {
        println!("{:<8} {path}", verb("update", "would update"));
    }
    for note in &report.skipped {
        println!("skip     {note}");
    }
    if dry_run && !report.diff.is_empty() {
        println!();
        print!("{}", report.diff);
    }
}
