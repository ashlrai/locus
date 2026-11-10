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

# Stop after both real allocations, exercising the EXIT trap entirely offline.
cat > "$test_root/spaces-shim.sh" <<'SHIM'
jq() { return 0; }
node() { return 42; }
mktemp() {
  local dir
  dir="$(command mktemp "$@")" || return
  printf '%s\n' "$dir" >> "$TEST_ROOT/allocated-dirs"
  printf '%s\n' "$dir"
}
SHIM

status=0
HOME="$test_root/home" TMPDIR="$test_root/tmp with spaces" TEST_ROOT="$test_root" \
  BASH_ENV="$test_root/spaces-shim.sh" bash "$ROOT/scripts/hub-integration-test.sh" \
  >"$test_root/spaces-stdout" 2>"$test_root/spaces-stderr" || status=$?

if [[ $status -ne 42 || ! -f "$test_root/allocated-dirs" ]]; then
  echo 'FAIL: expected both allocations before the offline stop' >&2
  exit 1
fi
home_count=0
project_count=0
while IFS= read -r dir; do
  case "$dir" in
    "$test_root/tmp with spaces"/locus-hub-int.*) home_count=$((home_count + 1)) ;;
    "$test_root/tmp with spaces"/locus-hub-int-proj.*) project_count=$((project_count + 1)) ;;
    *) echo 'FAIL: smoke directory allocated outside the spaced parent' >&2; exit 1 ;;
  esac
  if [[ -e "$dir" ]]; then
    echo 'FAIL: smoke directory under spaced parent was not removed' >&2
    exit 1
  fi
done < "$test_root/allocated-dirs"
if [[ $home_count -ne 1 || $project_count -ne 1 ]] || [[ -n "$(ls -A "$test_root/tmp with spaces")" ]]; then
  echo 'FAIL: expected two allocations and an empty spaced parent' >&2
  exit 1
fi
echo 'ok: both smoke directories under a spaced parent are cleaned up'
