#!/usr/bin/env bash
# Offline regression test for the workspace lifecycle in e2e.sh.
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
fixture="$(mktemp -d "${TMPDIR:-/tmp}/locus-workspace-test.XXXXXX")"
trap 'rm -rf "$fixture"' EXIT
root="$fixture/custom tmp  root"
mkdir -p "$root" "$fixture/home"
touch "$root/keep"

# Load the production functions without executing the network-capable suite.
for name in e2e_cleanup e2e_install_traps create_workspace create_locus_home create_graph_home create_security_home create_security_workspace; do
  definition="$(sed -n "/^${name}() {/,/^}/p" "$script_dir/e2e.sh")"
  [[ -n "$definition" ]] || { printf 'Missing %s\n' "$name" >&2; exit 1; }
  eval "$definition"
done

(
  export TMPDIR="$root"
  LOCUS_HOME="$fixture/home"
  WS_DIR="" GRAPH_HOME="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
  trap 'e2e_cleanup' EXIT
  create_workspace
  [[ -d "$WS_DIR" && "${WS_DIR%/*}" == "$TMPDIR" && "${WS_DIR##*/}" == locus-ws.* ]]
  printf '%s' "$WS_DIR" > "$fixture/workspace-path"
  mkdir "$WS_DIR/nested directory"
  touch "$WS_DIR/nested directory/sentinel"
)
workspace="$(cat "$fixture/workspace-path")"
[[ ! -e "$workspace" && ! -L "$workspace" && -f "$root/keep" && -d "$root" ]]
printf 'ok: custom TMPDIR workspace is created and cleaned on exit\n'

# Exercise every suite allocator together under a spaced root, and verify the
# shared exit trap removes each allocation without removing the caller's root.
(
  export TMPDIR="$root"
  WS_DIR="" GRAPH_HOME="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
  die() { exit 1; }
  create_locus_home
  trap 'e2e_cleanup' EXIT
  create_workspace
  create_graph_home
  create_security_home
  create_security_workspace
  for path in "$LOCUS_HOME" "$WS_DIR" "$GRAPH_HOME" "$SECURITY_HOME" "$SECURITY_WS"; do
    [[ -d "$path" && "${path%/*}" == "$TMPDIR" ]]
    printf '%s\n' "$path" >> "$fixture/allocated-paths"
    touch "$path/sentinel"
  done
)
while IFS= read -r path; do
  [[ ! -e "$path" && ! -L "$path" ]]
done < "$fixture/allocated-paths"
[[ $(wc -l < "$fixture/allocated-paths" | tr -d ' ') == 5 && -f "$root/keep" ]]
printf 'ok: all temporary directories honor spaced TMPDIR and are cleaned\n'

# Run the production interruption rehearsal offline. Its readiness handshake
# ensures SIGTERM arrives only after the child has created its temporary paths.
(
  LOCUS_HOME="$fixture/interrupt-parent"
  mkdir "$LOCUS_HOME"
  log() { :; }
  ok() { :; }
  die() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }
  # shellcheck source=scripts/e2e.d/90-workspace-interruption.sh
  source "$script_dir/e2e.d/90-workspace-interruption.sh"
)
printf 'ok: interruption removes temporary paths\n'

# Offline regression: failure of the first home allocation stops before Locus.
(
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
)
printf 'ok: home allocation failure stops before Locus\n'
