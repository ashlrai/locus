#!/usr/bin/env bash
# Regression: a failed first allocation must stop the smoke before any work.
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
test_dir="$(mktemp -d "${TMPDIR:-/tmp}/hub-smoke-first-mktemp.XXXXXX")"
trap 'rm -rf -- "$test_dir"' EXIT

cat >"$test_dir/stubs.sh" <<'EOF'
mktemp() { printf 'mktemp\n' >>"$TEST_COMMANDS"; return 1; }
locus() { printf 'locus\n' >>"$TEST_COMMANDS"; return 1; }
cargo() { printf 'cargo\n' >>"$TEST_COMMANDS"; return 1; }
jq() { printf 'jq\n' >>"$TEST_COMMANDS"; return 1; }
od() { printf 'od\n' >>"$TEST_COMMANDS"; return 1; }
tr() { printf 'tr\n' >>"$TEST_COMMANDS"; return 1; }
EOF

status=0
TEST_COMMANDS="$test_dir/commands" BASH_ENV="$test_dir/stubs.sh" \
  bash "$root/scripts/hub-smoke.sh" >"$test_dir/stdout" 2>"$test_dir/stderr" || status=$?

[[ $status -ne 0 ]] || { echo 'expected a nonzero exit' >&2; exit 1; }
[[ "$(cat "$test_dir/commands")" == mktemp ]] || { echo 'unexpected smoke command sequence' >&2; exit 1; }
[[ ! -s "$test_dir/stdout" ]] || { echo 'unexpected smoke output' >&2; exit 1; }
grep -Fxq 'error: hub-smoke could not create temporary home' "$test_dir/stderr"
