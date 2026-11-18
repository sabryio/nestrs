//! Compile-fail tests: the macros and trait bounds must explain themselves.
//!
//! The expected `.stderr` files are rustc's exact wording, which changes between
//! compiler releases, so this test is ignored by default and run on the pinned
//! toolchain: `cargo test -p rust-fx --test ui -- --ignored`
//! (regenerate with `TRYBUILD=overwrite`).
#![allow(missing_docs)]

#[test]
#[ignore = "depends on rustc's exact diagnostics; run on the pinned toolchain"]
fn diagnostics() {
    let cases = trybuild::TestCases::new();
    cases.compile_fail("tests/ui/*.rs");
}
