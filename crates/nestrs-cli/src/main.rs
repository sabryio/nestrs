//! The `nestrs` command line.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use nestrs_cli::commands::Options;
use nestrs_cli::templates::DependencySource;
use nestrs_cli::{DiskWorkspace, Report, commands};

#[derive(Parser)]
#[command(name = "nestrs", version, about = "Scaffold nestrs projects and features")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Args, Clone, Copy)]
struct Common {
    /// Overwrite files that already exist.
    #[arg(long)]
    force: bool,
    /// Show what would be written without writing.
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
        /// Use a local checkout of nestrs instead of the published crates.
        #[arg(long, value_name = "DIR")]
        nestrs_path: Option<String>,
        #[command(flatten)]
        common: Common,
    },
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
    /// A scheduled job (`nestrs-jobs`), registered in `AppModule` with its `JobsModule`.
    Job {
        /// Name, e.g. `purge-drafts`.
        name: String,
        #[command(flatten)]
        common: Common,
    },
    /// A queue consumer and its message type (`nestrs-jobs`), registered like a job.
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
        Command::New {
            name,
            nestrs_path,
            common,
        } => {
            let source = nestrs_path.map_or_else(
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
            println!("\nNext:\n  cd {name}\n  nestrs generate feature <name>\n  cargo run");
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
}
