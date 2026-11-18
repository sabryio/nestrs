# Releasing

All published crates share one version and are released together, in dependency order.

## Before the first release: crate names

crates.io names are global. Check every name in the table below is available **before** tagging:

```sh
for n in $(scripts/publish-order.sh); do
  curl -s -o /dev/null -w "$n %{http_code}\n" "https://index.crates.io/$(scripts/index-path.sh "$n")"
done   # 404 = free, 200 = taken
```

If a name is taken, rename the crate (package name and the workspace dependency key) before
publishing; nothing else in this document changes.

## Publish order

`scripts/publish-order.sh` prints it, computed from `cargo metadata` (a crate comes after every
crate it depends on, dev-dependencies with a version included), so it cannot drift from the
manifests. `purlin-bench` and the examples are `publish = false` and are skipped.

## Checklist

1. Move the entries under **Unreleased** in `CHANGELOG.md` to a new version heading.
2. Bump `[workspace.package] version` and every `version = ".."` in `[workspace.dependencies]`.
3. `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings`
4. `cargo test --workspace` (with `redis-server` installed and `REQUIRE_REDIS=1`)
5. `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --exclude purlin-bench`
6. `cargo semver-checks --workspace --exclude purlin-bench --exclude purlin-cli --exclude blog-api --exclude todo-minimal`
   (`purlin-cli`'s library is internal and not covered by semver.)
7. Tag `vX.Y.Z` and push. The `release` workflow re-runs the checks and publishes in order.

## Public API rules

- Anything not re-exported from a crate root, or marked `#[doc(hidden)]`, is not API.
- Macro support items live under `#[doc(hidden)]` and may change in any release.
- A new required trait method is a breaking change; give new methods a default body.
