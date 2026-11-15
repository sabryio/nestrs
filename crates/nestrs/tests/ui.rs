//! Compile-fail tests pin the macros' diagnostics: every misuse should produce a
//! short, specific message pointing at the offending tokens. Regenerate the expected
//! output with `TRYBUILD=overwrite cargo test -p nestrs --test ui` and review the diff.

#[test]
fn macro_diagnostics() {
    let cases = trybuild::TestCases::new();
    cases.pass("tests/ui/pass/*.rs");
    cases.compile_fail("tests/ui/fail/*.rs");
}
