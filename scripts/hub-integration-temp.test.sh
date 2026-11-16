#!/usr/bin/env bash
# Run: bash scripts/hub-integration-temp.test.sh
# hub-integration-test.sh must use two distinct, non-overlapping temporary
# directories for its home and its project. Stub every tool so no Locus runs.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
script="$root/scripts/hub-integration-test.sh"
fixture="$(mktemp -d "${TMPDIR:-/tmp}/hub-int-temp-test.XXXXXX")"
trap 'rm -rf -- "$fixture"' EXIT
mkdir -p "$fixture/tmp with spaces"

# Record SMOKE_HOME and SMOKE_PROJ as seen at `locus init`, then abort.
harness='
node() { :; }; jq() { :; }; cargo() { :; }
locus() {
  if [[ "$1" == init ]]; then
    printf "%s\n" "$SMOKE_HOME" "$SMOKE_PROJ" > "$RECORD"
    return 73
  fi
}
export -f node jq cargo locus
exec bash "$1"'

status=0
env BASH_ENV= HOME="$HOME" PATH="$PATH" TMPDIR="$fixture/tmp with spaces" \
  RECORD="$fixture/record" bash -c "$harness" _ "$script" >/dev/null 2>&1 || status=$?
[[ "$status" -eq 73 ]] || { echo "FAIL: expected init to be reached (exit 73), got $status" >&2; exit 1; }

{ IFS= read -r smoke_home; IFS= read -r smoke_proj; } < "$fixture/record"
[[ -n "$smoke_home" && -n "$smoke_proj" ]] || { echo "FAIL: paths not recorded" >&2; exit 1; }
[[ "$smoke_home" != "$smoke_proj" ]] || { echo "FAIL: home and project are the same path" >&2; exit 1; }
case "$smoke_home/" in "$smoke_proj"/*) echo "FAIL: home is inside project" >&2; exit 1 ;; esac
case "$smoke_proj/" in "$smoke_home"/*) echo "FAIL: project is inside home" >&2; exit 1 ;; esac

# Both were directories when init ran (cleanup has since removed them), so
# re-run with a stub that checks while they still exist.
harness_dir='
node() { :; }; jq() { :; }; cargo() { :; }
locus() { [[ "$1" == init ]] && { [[ -d "$SMOKE_HOME" && -d "$SMOKE_PROJ" ]] && return 73 || return 74; }; }
export -f node jq cargo locus
exec bash "$1"'
status=0
env BASH_ENV= HOME="$HOME" PATH="$PATH" TMPDIR="$fixture/tmp with spaces" \
  bash -c "$harness_dir" _ "$script" >/dev/null 2>&1 || status=$?
[[ "$status" -eq 73 ]] || { echo "FAIL: home and project must both be directories (got $status)" >&2; exit 1; }

[[ -z "$(ls -A "$fixture/tmp with spaces")" ]] || { echo "FAIL: temporary directories left behind" >&2; exit 1; }
echo "ok: hub-integration temp home and project are distinct, non-overlapping directories"
