#!/usr/bin/env bash
# Proves that the headless crates really are headless: none of them may pull the HTTP stack
# into its normal dependency tree.
#
#   scripts/check-headless.sh
set -euo pipefail

FORBIDDEN='^(axum|hyper|hyper-util|tower|tower-http|http-body|tokio-tungstenite|purlin-http|utoipa)$'

# package, extra cargo flags
CHECKS=(
  "rust-fx|"
  "headless-worker|"
  "purlin-jobs|"
  "purlin-cache|"
  "purlin-outbox|"
  "purlin-health|--no-default-features"
  "purlin-sea-orm|--no-default-features"
)

status=0
for entry in "${CHECKS[@]}"; do
  package="${entry%%|*}"
  flags="${entry#*|}"
  # shellcheck disable=SC2086
  leaked=$(cargo tree -p "$package" $flags -e normal --prefix none 2>/dev/null | awk '{print $1}' | sort -u | grep -E "$FORBIDDEN" || true)
  if [ -n "$leaked" ]; then
    echo "FAIL  $package $flags pulls: $(echo "$leaked" | tr '\n' ' ')"
    status=1
  else
    echo "ok    $package $flags"
  fi
done
exit $status
