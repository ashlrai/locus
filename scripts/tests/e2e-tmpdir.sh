#!/usr/bin/env bash
# Exercise the full suite with a nondefault TMPDIR and verify its cleanup.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TEST_DIR="$(mktemp -d "${TMPDIR:-/tmp}/locus-tmpdir-regression.XXXXXX")"
trap 'rm -rf "$TEST_DIR"' EXIT
mkdir -p "$TEST_DIR/custom tmp"

# Keep suite output private: some tests inspect process environments and tokens.
if ! TMPDIR="$TEST_DIR/custom tmp" bash "$ROOT/scripts/e2e.sh" >"$TEST_DIR/e2e.log" 2>&1; then
  printf 'FAIL: e2e suite failed with a nondefault temporary directory\n' >&2
  exit 1
fi
for check in \
  'ok  security workspace created beneath temporary directory' \
  'ok  security workspace cleaned up'; do
  grep -Fq "$check" "$TEST_DIR/e2e.log" || {
    printf 'FAIL: e2e suite did not report the expected workspace checks\n' >&2
    exit 1
  }
done
grep -Ex 'e2e PASS  \([0-9]+ checks, [0-9]+ skipped\)' "$TEST_DIR/e2e.log" || {
  printf 'FAIL: e2e suite did not report its summary\n' >&2
  exit 1
}
if [[ -n "$(find "$TEST_DIR/custom tmp" -mindepth 1 -print -quit)" ]]; then
  printf 'FAIL: e2e suite left files in the temporary directory\n' >&2
  exit 1
fi
printf 'PASS: nondefault TMPDIR workspace creation and cleanup\n'
