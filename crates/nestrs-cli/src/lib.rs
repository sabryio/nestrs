//! # nestrs-cli
//!
//! The engine behind the `nestrs` command. Generation is split so each part has one job
//! and can be tested without a disk:
//!
//! | Module | Job |
//! |---|---|
//! | [`naming`] | turns `invoice-line` into every spelling a template needs (and refuses bad names) |
//! | [`templates`] | *pure*: a name in, a [`Plan`] of file operations out |
//! | [`patch`] | edits existing Rust by parsing it (`syn` spans), never by matching text |
//! | [`plan`] | the operations ([`Op`]) and how they are applied, safely and idempotently |
//! | [`workspace`] | the [`Workspace`] port (read / write files) with disk and in-memory adapters |
//! | [`contract`] | `generate contract`: the TypeScript oRPC contract, read from the Rust sources with `syn` |
//! | [`doctor`] | `doctor`: layer-rule and wiring checks, by parsing |
//! | [`diff`] | unified diffs for `--dry-run` and `--check` |
//! | [`commands`] | glues them: resolve names, build the plan, apply it, report |
//!
//! ## Stability
//!
//! The supported interface of this crate is the `nestrs` command. The library exists so the
//! engine can be tested and embedded in tooling, but it is **not covered by semver**: modules
//! and signatures may change in any release. (The release workflow excludes it from the
//! semver check for that reason.)
//!
//! Adding a generator means adding a function in [`templates`]; nothing else changes (OCP).
//! Generators never touch the file system (DIP), so they are plain-data tests.

#![forbid(unsafe_code)]

pub mod commands;
pub mod contract;
pub mod diff;
pub mod doctor;
pub mod naming;
pub mod patch;
pub mod plan;
pub mod templates;
pub mod workspace;

pub use commands::{CliError, new_feature, new_project};
pub use naming::{NameError, Names};
pub use plan::{Op, Plan, Report};
pub use workspace::{DiskWorkspace, MemoryWorkspace, Workspace};
