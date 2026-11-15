//! The strongest check a generator has: what it writes compiles, passes its own tests and
//! is clippy-clean. Slow (it builds a project), so it only runs on request:
//!
//! ```text
//! cargo test -p nestrs-cli -- --ignored
//! ```

use std::path::Path;
use std::process::Command;

use nestrs_cli::DiskWorkspace;
use nestrs_cli::commands::{Options, new_feature, new_job, new_module, new_project, new_queue_handler};
use nestrs_cli::templates::DependencySource;

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

    // Share the workspace's build cache so the dependencies are not compiled again.
    let target = repo.join("target");
    cargo(&project, &target, &["test", "--quiet"]);
    cargo(&project, &target, &["clippy", "--all-targets", "--", "-D", "warnings"]);
}
