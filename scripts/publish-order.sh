#!/usr/bin/env bash
# Prints the publishable crates in dependency order, one per line.
# `cargo metadata` is the source of truth for what is publishable; the order is a topological
# sort over normal and build dependencies, plus dev-dependencies that carry a version.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo metadata --format-version 1 --no-deps | python3 -c '
import json, sys
m = json.load(sys.stdin)
pk = {p["name"]: p for p in m["packages"] if p.get("publish") != []}
deps = {n: set() for n in pk}
for n, p in pk.items():
    for d in p["dependencies"]:
        if d["name"] in pk and d.get("req") not in (None, "*"):
            deps[n].add(d["name"])
order, done = [], set()
while len(done) < len(pk):
    ready = sorted(n for n in pk if n not in done and deps[n] <= done)
    if not ready:
        sys.exit("dependency cycle among: " + ", ".join(sorted(set(pk) - done)))
    order += ready
    done |= set(ready)
print("\n".join(order))
'
