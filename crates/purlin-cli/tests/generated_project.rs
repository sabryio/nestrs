//! The strongest check a generator has: what it writes compiles, passes its own tests and
//! is clippy-clean. Slow (it builds a project), so it only runs on request:
//!
//! ```text
//! cargo test -p purlin-cli -- --ignored
//! ```

use std::path::Path;
use std::process::Command;

use purlin_cli::DiskWorkspace;
use purlin_cli::commands::{
    Options, new_controller, new_feature, new_job, new_module, new_port, new_project, new_queue_handler, new_use_case,
};
use purlin_cli::templates::DependencySource;

fn cargo(dir: &Path, target: &Path, args: &[&str]) {
    let output = Command::new(env!("CARGO"))
        .args(args)
        .current_dir(dir)
        .env("CARGO_TARGET_DIR", target)
        .output()
        .expect("cargo runs");
    assert!(
        output.status.success(),
        "cargo {args:?} failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "builds a generated project; run with --ignored"]
fn a_generated_project_with_features_builds_tests_and_lints_clean() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("shop-api");

    let mut workspace = DiskWorkspace::new(&project);
    let source = DependencySource::Path(repo.display().to_string());
    new_project(&mut workspace, "shop-api", &source, Options::default()).unwrap();
    new_feature(&mut workspace, "invoice", None, Options::default()).unwrap();
    new_feature(&mut workspace, "category", None, Options::default()).unwrap();
    new_module(&mut workspace, "billing", Options::default()).unwrap();
    new_job(&mut workspace, "purge-drafts", Options::default()).unwrap();
    new_queue_handler(&mut workspace, "send-welcome", Options::default()).unwrap();
    new_use_case(&mut workspace, "invoice", None, "archive", Options::default()).unwrap();
    new_controller(&mut workspace, "billing", Some("billing"), Options::default()).unwrap();
    new_port(&mut workspace, "mailer", Options::default()).unwrap();

    // Everything generated obeys the architecture it was generated for.
    let diagnosis = purlin_cli::doctor::diagnose(&workspace).unwrap();
    assert!(diagnosis.is_healthy(), "{:?}", diagnosis.violations);

    // And the API it exposes can be described as a contract without leaving anything out.
    let report = purlin_cli::contract::run(
        &mut workspace,
        &purlin_cli::contract::ContractOptions {
            out: "contract.generated.ts".into(),
            strict: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(report.routes >= 6, "{report:?}");

    // Share the workspace's build cache so the dependencies are not compiled again.
    let target = repo.join("target");
    cargo(&project, &target, &["test", "--quiet"]);
    cargo(&project, &target, &["clippy", "--all-targets", "--", "-D", "warnings"]);
}
