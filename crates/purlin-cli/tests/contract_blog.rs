//! The blog example's contract, generated from its real sources. Skipped when the example is not
//! next to this crate (a packaged crate).

use std::path::Path;

use purlin_cli::contract::{ContractOptions, run};
use purlin_cli::workspace::DiskWorkspace;

#[test]
fn the_committed_contract_matches_the_blog_sources() {
    let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/blog-api");
    let committed = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/react-orpc/src/contract.generated.ts");
    if !example.exists() || !committed.exists() {
        return;
    }
    let mut workspace = DiskWorkspace::new(&example);
    let report = run(
        &mut workspace,
        &ContractOptions {
            out: committed,
            also: vec![Path::new("../../crates/purlin-health/src").to_owned()],
            strict: true,
            check: true,
            ..ContractOptions::default()
        },
    )
    .unwrap_or_else(|error| panic!("the committed contract is stale: {error}"));
    assert_eq!((report.routes, report.schemas), (10, 9));
}
