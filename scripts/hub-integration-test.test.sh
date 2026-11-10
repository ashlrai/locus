#!/usr/bin/env bash
# Offline regression for cleanup when the second smoke directory cannot be made.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/locus-hub-test.XXXXXX")"
trap 'rm -rf -- "$test_root"' EXIT
mkdir -p "$test_root/tmp with spaces" "$test_root/home"

# BASH_ENV supplies the shim to the real script, including its command
# substitutions. Record the first allocation outside the cleanup target.
cat > "$test_root/shim.sh" <<'SHIM'
jq() { return 0; }
node() { return 0; }
mktemp() {
  case "$2" in
    */locus-hub-int.XXXXXX)
      local dir
      dir="$(command mktemp "$@")" || return
      printf '%s\n' "$dir" > "$TEST_ROOT/first-dir"
      printf '%s\n' "$dir"
      ;;
    */locus-hub-int-proj.XXXXXX)
      printf '%s\n' reached > "$TEST_ROOT/second-attempt"
      return 1
      ;;
    *) return 1 ;;
  esac
}
SHIM

status=0
HOME="$test_root/home" TMPDIR="$test_root/tmp with spaces" TEST_ROOT="$test_root" \
  BASH_ENV="$test_root/shim.sh" bash "$ROOT/scripts/hub-integration-test.sh" \
  >"$test_root/stdout" 2>"$test_root/stderr" || status=$?

if [[ $status -eq 0 || ! -s "$test_root/first-dir" || ! -f "$test_root/second-attempt" ]]; then
  echo 'FAIL: expected the second directory allocation to fail' >&2
  exit 1
fi
IFS= read -r first_dir < "$test_root/first-dir"
if [[ -e "$first_dir" ]]; then
  echo 'FAIL: first smoke directory was not removed' >&2
  exit 1
fi
echo 'ok: second directory allocation failure cleans up the first directory'
