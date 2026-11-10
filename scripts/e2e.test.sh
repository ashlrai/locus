#!/usr/bin/env bash
# Offline regression: failure of the first home allocation stops before Locus.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
test_dir="$(mktemp -d "${TMPDIR:-/tmp}/locus-e2e-failure-test.XXXXXX")"
trap 'rm -rf -- "$test_dir"' EXIT

cat >"$test_dir/stubs.sh" <<'STUBS'
cargo() { return 0; }
mktemp() { printf 'attempted\n' >"$TEST_DIR/mktemp-called"; return 73; }
STUBS
cat >"$test_dir/app" <<'APP'
#!/usr/bin/env bash
printf 'invoked\n' >>"$TEST_DIR/app-called"
exit 0
APP
chmod +x "$test_dir/app"

status=0
TEST_DIR="$test_dir" BASH_ENV="$test_dir/stubs.sh" \
  LOCUS_CONTROL_CAPABILITY=synthetic LOCUS_BIN="$test_dir/app" MCP_BIN="$test_dir/app" \
  bash "$root/scripts/e2e.sh" >"$test_dir/stdout" 2>"$test_dir/stderr" || status=$?

[[ $status -ne 0 ]] || { echo 'home allocation failure returned success' >&2; exit 1; }
[[ -f "$test_dir/mktemp-called" ]] || { echo 'home allocation was not attempted' >&2; exit 1; }
[[ ! -e "$test_dir/app-called" ]] || { echo 'application ran after home allocation failure' >&2; exit 1; }
grep -Fq 'FAIL could not create LOCUS_HOME' "$test_dir/stderr" || {
  echo 'home allocation failure was not reported' >&2; exit 1;
}
