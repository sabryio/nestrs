//! Enforces the dependency rule: source dependencies point inward only.
//!
//! domain -> (nothing) · application -> domain · infrastructure -> application, domain
//! presentation -> application, domain · composition -> everything

use std::fs;
use std::path::{Path, PathBuf};

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Code lines of a layer (comments skipped), with their file and line number.
fn code_lines(layer: &str) -> Vec<(String, usize, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join(layer);
    let mut files = Vec::new();
    rust_files(&root, &mut files);
    assert!(!files.is_empty(), "layer `{layer}` has no files");
    let mut lines = Vec::new();
    for file in files {
        for (number, line) in fs::read_to_string(&file).unwrap().lines().enumerate() {
            let code = line.trim();
            if !code.starts_with("//") {
                lines.push((file.display().to_string(), number + 1, code.to_owned()));
            }
        }
    }
    lines
}

fn assert_no_mentions(layer: &str, forbidden: &[&str]) {
    assert_no_mentions_except(layer, forbidden, &[]);
}

/// Like [`assert_no_mentions`], but lines that contain one of `allowed` are exempt.
fn assert_no_mentions_except(layer: &str, forbidden: &[&str], allowed: &[&str]) {
    let mut violations = Vec::new();
    for (file, number, code) in code_lines(layer) {
        if allowed.iter().any(|ok| code.contains(ok)) {
            continue;
        }
        for needle in forbidden {
            if code.contains(needle) {
                violations.push(format!("{file}:{number} mentions `{needle}`: {code}"));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "`{layer}` breaks the dependency rule:\n{}",
        violations.join("\n")
    );
}

const FRAMEWORKS: [&str; 5] = ["nestrs", "axum", "serde", "validator", "tower"];

#[test]
fn the_domain_depends_on_nothing_outside_itself() {
    let mut forbidden = FRAMEWORKS.to_vec();
    forbidden.extend([
        "crate::application",
        "crate::infrastructure",
        "crate::presentation",
        "crate::composition",
    ]);
    // The one exception: the declarative `#[port]` attribute, which only marks a
    // trait as an injectable abstraction (and brings in `async_trait`).
    assert_no_mentions_except("domain", &forbidden, &["use nestrs::port;", "#[port]"]);
}

#[test]
fn the_application_depends_on_the_domain_only() {
    let mut forbidden = FRAMEWORKS.to_vec();
    forbidden.extend(["crate::infrastructure", "crate::presentation", "crate::composition"]);
    // Declarative DI attributes only: `#[port]` and `#[injectable]`.
    assert_no_mentions_except(
        "application",
        &forbidden,
        &[
            "use nestrs::port;",
            "use nestrs::injectable;",
            "#[port]",
            "#[injectable]",
        ],
    );
}

#[test]
fn infrastructure_does_not_reach_outward() {
    assert_no_mentions("infrastructure", &["crate::presentation", "crate::composition", "axum"]);
}

#[test]
fn presentation_never_names_an_adapter() {
    assert_no_mentions("presentation", &["crate::infrastructure", "crate::composition"]);
}
