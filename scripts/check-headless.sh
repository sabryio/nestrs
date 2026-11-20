#!/usr/bin/env bash
# Proves that the headless crates really are headless: none of them may pull the HTTP stack
# into its normal dependency tree.
#
#   scripts/check-headless.sh
set -euo pipefail

FORBIDDEN='^(axum|hyper|hyper-util|tower|tower-http|http-body|tokio-tungstenite|purlin-http|utoipa)$'

# Per-package exceptions, as a regex of crates that may appear anyway. purlin-otel exports over
# OTLP/HTTP, so it legitimately has an HTTP *client* (reqwest: hyper, tower, ...); what it must
# not have is the HTTP *server* stack, so axum, purlin-http and utoipa stay forbidden.
declare -A ALLOW=(
  ["purlin-otel"]='^(hyper|hyper-util|tower|tower-http|http-body)$'
)

# package, extra cargo flags
CHECKS=(
  "rust-fx|"
  "headless-worker|"
  "purlin-jobs|"
  "purlin-cache|"
  "purlin-outbox|"
  "purlin-health|--no-default-features"
  "purlin-sea-orm|--no-default-features"
  "purlin-fx|--no-default-features"
  "purlin-otel|--no-default-features"
)

status=0
for entry in "${CHECKS[@]}"; do
  package="${entry%%|*}"
  flags="${entry#*|}"
  # shellcheck disable=SC2086
  leaked=$(cargo tree -p "$package" $flags -e normal --prefix none 2>/dev/null | awk '{print $1}' | sort -u | grep -E "$FORBIDDEN" || true)
  if [ -n "${ALLOW[$package]:-}" ] && [ -n "$leaked" ]; then
    leaked=$(echo "$leaked" | grep -Ev "${ALLOW[$package]}" || true)
  fi
  if [ -n "$leaked" ]; then
    echo "FAIL  $package $flags pulls: $(echo "$leaked" | tr '\n' ' ')"
    status=1
  else
    echo "ok    $package $flags"
  fi
done

# One-way dependency. purlin runs on rust-fx (purlin-core executes its module graph on `fx::App`),
# so purlin -> fx is allowed; fx -> purlin never is. Within purlin, only `purlin-core` may name
# rust-fx directly: everything above reaches it through purlin-core, so the engine can be
# replaced in one place.
direct_fx=$(cargo metadata --no-deps --format-version 1 | python3 -c '
import json, sys
for p in json.load(sys.stdin)["packages"]:
    if p["name"].startswith("purlin") and p["name"] not in ("purlin-core", "purlin-fx", "purlin-on-fx", "purlin-bench"):
        if any(d["name"] in ("rust-fx", "rust-fx-macros") and d.get("kind") in (None, "normal") for d in p["dependencies"]):
            print(p["name"])')
if [ -n "$direct_fx" ]; then
  echo "FAIL  these purlin crates name rust-fx directly (go through purlin-core):" $direct_fx; status=1
else
  echo "ok    only purlin-core (and the adapter) name rust-fx directly"
fi

deps() { cargo tree -p "$1" -e normal --prefix none 2>/dev/null | awk '{print $1}' | sort -u; }

# fx never knows purlin.
for package in rust-fx rust-fx-macros headless-worker fx-axum; do
  found=$(deps "$package" | grep -E '^purlin' || true)
  if [ -n "$found" ]; then
    echo "FAIL  $package depends on: $(echo "$found" | tr '\n' ' ')"; status=1
  else
    echo "ok    $package does not know purlin"
  fi
done
exit $status
