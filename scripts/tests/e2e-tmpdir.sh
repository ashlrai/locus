#!/usr/bin/env bash
# Exercise the real e2e workspace and its EXIT cleanup under a custom TMPDIR.
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
case "$created" in
  */locus-ws.*)
    [[ -d "$created" ]] || exit 1
    printf '%s\n' "$created" >>"$E2E_WORKSPACE_RECORD"
    ;;
esac
printf '%s\n' "$created"
EOF
chmod +x "$TEST_ROOT/bin/mktemp"

export E2E_REAL_MKTEMP="$REAL_MKTEMP"
export E2E_WORKSPACE_RECORD="$TEST_ROOT/workspaces"
export TMPDIR="$TEST_ROOT/custom tmp"
export PATH="$TEST_ROOT/bin:$PATH"

if ! bash "$ROOT/scripts/e2e.sh" >"$TEST_ROOT/e2e.log" 2>&1; then
  printf 'FAIL: e2e suite failed under a custom temporary directory\n' >&2
  exit 1
fi

[[ -f "$E2E_WORKSPACE_RECORD" ]] || { printf 'FAIL: workspace was not created\n' >&2; exit 1; }
workspaces=()
while IFS= read -r workspace; do
  workspaces+=("$workspace")
done <"$E2E_WORKSPACE_RECORD"
[[ ${#workspaces[@]} -eq 1 ]] || { printf 'FAIL: expected one workspace\n' >&2; exit 1; }
workspace="${workspaces[0]}"
[[ "${workspace%/*}" == "$TMPDIR" ]] || { printf 'FAIL: workspace was not created directly beneath TMPDIR\n' >&2; exit 1; }
[[ ! -e "$workspace" && ! -L "$workspace" ]] || { printf 'FAIL: workspace was not cleaned up\n' >&2; exit 1; }
grep '^e2e PASS ' "$TEST_ROOT/e2e.log"
printf 'PASS: custom TMPDIR workspace was created and cleaned up\n'
