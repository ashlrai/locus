#!/usr/bin/env bash
# A failed first allocation must prevent all integration commands.
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
test_dir="$(mktemp -d "${TMPDIR:-/tmp}/hub-integration-first-mktemp.XXXXXX")"
trap 'rm -rf -- "$test_dir"' EXIT

cat >"$test_dir/stubs.sh" <<'STUBS'
mktemp() { printf 'mktemp\n' >>"$TEST_COMMANDS"; return 73; }
node() { printf 'node\n' >>"$TEST_COMMANDS"; return 0; }
locus() { printf 'locus\n' >>"$TEST_COMMANDS"; return 0; }
cargo() { printf 'cargo\n' >>"$TEST_COMMANDS"; return 0; }
jq() { printf 'jq\n' >>"$TEST_COMMANDS"; return 0; }
STUBS

status=0
TEST_COMMANDS="$test_dir/commands" BASH_ENV="$test_dir/stubs.sh" \
  LOCUS_CONTROL_CAPABILITY=synthetic \
  bash "$root/scripts/hub-integration-test.sh" >"$test_dir/stdout" 2>"$test_dir/stderr" || status=$?

[[ $status -eq 73 ]] || { echo 'first mktemp failure did not propagate' >&2; exit 1; }
[[ "$(cat "$test_dir/commands")" == mktemp ]] || { echo 'integration command ran after allocation failure' >&2; exit 1; }
[[ ! -s "$test_dir/stdout" ]] || { echo 'unexpected integration output' >&2; exit 1; }
