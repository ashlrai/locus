#!/usr/bin/env bash
# Exercise the real e2e home and workspace cleanup under a custom TMPDIR.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
REAL_MKTEMP="$(command -v mktemp)"
TEST_ROOT="$("$REAL_MKTEMP" -d "${TMPDIR:-/tmp}/locus-tmpdir-test.XXXXXX")"
trap 'rm -rf "$TEST_ROOT"' EXIT
mkdir -p "$TEST_ROOT/custom tmp" "$TEST_ROOT/bin"

# Record the directory made by the actual e2e invocation, and check that it
# exists at creation time. Everything else passes through to the real mktemp.
cat >"$TEST_ROOT/bin/mktemp" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
created="$("$E2E_REAL_MKTEMP" "$@")"
# Match the template: a created-path glob also matches nested security fixtures.
case "${2:-}" in
  */locus-e2e.XXXXXX)
    [[ -d "$created" ]] || exit 1
    printf '%s\n' "$created" >>"$E2E_HOME_RECORD"
    ;;
  */locus-ws.XXXXXX)
    [[ -d "$created" ]] || exit 1
    printf '%s\n' "$created" >>"$E2E_WORKSPACE_RECORD"
    ;;
esac
printf '%s\n' "$created"
EOF
chmod +x "$TEST_ROOT/bin/mktemp"

export E2E_REAL_MKTEMP="$REAL_MKTEMP"
export E2E_HOME_RECORD="$TEST_ROOT/homes"
export E2E_HOME_USED="$TEST_ROOT/home-used"
export E2E_WORKSPACE_RECORD="$TEST_ROOT/workspaces"
export TMPDIR="$TEST_ROOT/custom tmp"
export PATH="$TEST_ROOT/bin:$PATH"
touch "$TMPDIR/unrelated"

assert_fixtures_cleaned() {
  [[ -f "$E2E_HOME_RECORD" && -f "$E2E_HOME_USED" ]] || { printf 'FAIL: home was not created and used\n' >&2; exit 1; }
  local homes=() home
  while IFS= read -r home; do
    homes+=("$home")
  done <"$E2E_HOME_RECORD"
  [[ ${#homes[@]} -eq 1 ]] || { printf 'FAIL: expected one home\n' >&2; exit 1; }
  home="${homes[0]}"
  [[ "${home%/*}" == "$TMPDIR" ]] || { printf 'FAIL: home was not created directly beneath TMPDIR\n' >&2; exit 1; }
  [[ "$(cat "$E2E_HOME_USED")" == "$home" ]] || { printf 'FAIL: CLI used a different home\n' >&2; exit 1; }
  [[ ! -e "$home" && ! -L "$home" ]] || { printf 'FAIL: home was not cleaned up\n' >&2; exit 1; }
  [[ -f "$E2E_WORKSPACE_RECORD" ]] || { printf 'FAIL: workspace was not created\n' >&2; exit 1; }
  local workspaces=() workspace
  while IFS= read -r workspace; do
    workspaces+=("$workspace")
  done <"$E2E_WORKSPACE_RECORD"
  [[ ${#workspaces[@]} -eq 1 ]] || { printf 'FAIL: expected one workspace\n' >&2; exit 1; }
  workspace="${workspaces[0]}"
  [[ "${workspace%/*}" == "$TMPDIR" ]] || { printf 'FAIL: workspace was not created directly beneath TMPDIR\n' >&2; exit 1; }
  [[ ! -e "$workspace" && ! -L "$workspace" ]] || { printf 'FAIL: workspace was not cleaned up\n' >&2; exit 1; }
  [[ -f "$TMPDIR/unrelated" ]] || { printf 'FAIL: cleanup removed unrelated data\n' >&2; exit 1; }
  (
    shopt -s nullglob dotglob
    leftovers=("$TMPDIR"/*)
    [[ ${#leftovers[@]} -eq 1 && "${leftovers[0]}" == "$TMPDIR/unrelated" ]]
  ) || { printf 'FAIL: e2e left temporary files behind\n' >&2; exit 1; }
}

# Observe the home used by the actual CLI after setup, and optionally fail
# after the real workspace command has populated the workspace.
ln -s "${LOCUS_BIN:-$ROOT/target/release/locus}" "$TEST_ROOT/bin/real-locus"
cat >"$TEST_ROOT/bin/observe-locus" <<'EOF'
#!/bin/bash
if [[ "${1:-}" == workspace ]]; then
  IFS= read -r workspace <"$E2E_WORKSPACE_RECORD"
  [[ -d "$workspace" && "$PWD" -ef "$workspace" && -d "$LOCUS_HOME" ]] || exit 1
  printf '%s\n' "$LOCUS_HOME" >"$E2E_HOME_USED"
  touch "$LOCUS_HOME/cleanup-sentinel" "$PWD/cleanup-sentinel"
  if [[ "${E2E_INJECT_FAILURE:-}" == 1 ]]; then
    "${0%/*}/real-locus" "$@" || exit 1
    [[ -f "$workspace/.locus.toml" && -f "$workspace/cleanup-sentinel" && \
       -f "$LOCUS_HOME/cleanup-sentinel" ]] || exit 1
    printf '%s\n' "$workspace" >"$E2E_FAILURE_RECORD"
    exit 73
  fi
fi
exec "${0%/*}/real-locus" "$@"
EOF
chmod +x "$TEST_ROOT/bin/observe-locus"
export LOCUS_BIN="$TEST_ROOT/bin/observe-locus"

if ! bash "$ROOT/scripts/e2e.sh" >"$TEST_ROOT/e2e.log" 2>&1; then
  printf 'FAIL: e2e suite failed under a custom temporary directory\n' >&2
  exit 1
fi

assert_fixtures_cleaned
grep '^e2e PASS ' "$TEST_ROOT/e2e.log"
printf 'PASS: custom TMPDIR home and workspace were created and cleaned up\n'

# Fail the first command inside the real workspace after it creates its config.
# All earlier commands still use the real binary and exercise the real script.
export E2E_FAILURE_RECORD="$TEST_ROOT/failure-workspace"
rm "$E2E_WORKSPACE_RECORD" "$E2E_HOME_RECORD" "$E2E_HOME_USED"
status=0
E2E_INJECT_FAILURE=1 bash "$ROOT/scripts/e2e.sh" >"$TEST_ROOT/failure.log" 2>&1 || status=$?
[[ "$status" -eq 73 ]] || { printf 'FAIL: expected status 73 from injected failure, got %s\n' "$status" >&2; exit 1; }
[[ -f "$E2E_FAILURE_RECORD" ]] || { printf 'FAIL: injected failure was not reached\n' >&2; exit 1; }
[[ "$(cat "$E2E_FAILURE_RECORD")" == "$(cat "$E2E_WORKSPACE_RECORD")" ]] \
  || { printf 'FAIL: injected failure did not occur in the recorded workspace\n' >&2; exit 1; }
assert_fixtures_cleaned
printf 'PASS: home and workspace were cleaned up after an injected failure\n'
