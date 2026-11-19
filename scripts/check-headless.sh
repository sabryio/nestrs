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

# The two frameworks must not know each other. Only the adapter (`purlin-fx`) and the
# examples may depend on both.
deps() { cargo tree -p "$1" -e normal --prefix none 2>/dev/null | awk '{print $1}' | sort -u; }

fx_in_purlin=""
for package in $(cargo metadata --no-deps --format-version 1 | python3 -c '
import json, sys
for p in json.load(sys.stdin)["packages"]:
    if p["name"].startswith("purlin") and p["name"] not in ("purlin-fx", "purlin-bench", "purlin-on-fx"):
        print(p["name"])'); do
  found=$(deps "$package" | grep -E '^rust-fx(-macros)?$' || true)
  [ -n "$found" ] && fx_in_purlin="$fx_in_purlin $package"
done
if [ -n "$fx_in_purlin" ]; then
  echo "FAIL  these purlin crates depend on rust-fx:$fx_in_purlin"; status=1
else
  echo "ok    no purlin crate depends on rust-fx"
fi

for package in rust-fx rust-fx-macros headless-worker fx-axum; do
  found=$(deps "$package" | grep -E '^purlin' || true)
  if [ -n "$found" ]; then
    echo "FAIL  $package depends on: $(echo "$found" | tr '\n' ' ')"; status=1
  else
    echo "ok    $package does not know purlin"
  fi
done
exit $status
