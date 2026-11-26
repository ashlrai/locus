#!/usr/bin/env bash
# Offline regression tests for temporary home privacy and resource cleanup in e2e.sh.
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
for expected_status in 0 42; do
  paths="$fixture/allocated-paths-$expected_status"
  home_mode="$fixture/home-mode-$expected_status"
  allocation_args="$fixture/allocation-args-$expected_status"
  cleanup_args="$fixture/cleanup-args-$expected_status"
  # A separate shell keeps errexit active even while we capture its status.
  set +e
  (
    set -e
    export TMPDIR="$root"
    WS_DIR="" GRAPH_HOME="" GRAPH_HOMES="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
    die() { exit 1; }
    # Record argument boundaries, not $*: paths containing spaces must reach
    # both commands intact. Delegate only to local filesystem utilities.
    mktemp() {
      printf '%s\0' "$#" "$@" >> "$allocation_args"
      command mktemp "$@"
    }
    rm() {
      printf '%s\0' "$#" "$@" >> "$cleanup_args"
      command rm "$@"
    }
    # Even a permissive caller must get a private home for suite state.
    umask 000
    create_locus_home
    e2e_install_traps
    create_workspace
    create_graph_home
    create_security_home
    create_security_workspace
    [[ "$LOCUS_HOME" != "$WS_DIR" && ! "$LOCUS_HOME" -ef "$WS_DIR" ]]
    printf 'home' > "$LOCUS_HOME/sentinel"
    printf 'workspace' > "$WS_DIR/sentinel"
    [[ "$(cat "$LOCUS_HOME/sentinel")" == home && "$(cat "$WS_DIR/sentinel")" == workspace ]]
    # Record permissions while the populated home is in use, before EXIT
    # removes it. GNU and BSD stat use different flags for the octal mode.
    if stat -c '%a' "$LOCUS_HOME" > "$home_mode" 2>/dev/null; then
      :
    else
      stat -f '%Lp' "$LOCUS_HOME" > "$home_mode"
    fi
    for path in "$LOCUS_HOME" "$WS_DIR" "$GRAPH_HOME" "$SECURITY_HOME" "$SECURITY_WS"; do
      [[ -d "$path" && "${path%/*}" == "$TMPDIR" ]]
      printf '%s\n' "$path" >> "$paths"
      touch "$path/sentinel"
    done
    exit "$expected_status"
  )
  status=$?
  set -e
  [[ "$status" -eq "$expected_status" ]]
  for prefix in locus-e2e locus-ws locus-graph-imp locus-security-e2e locus-security-ws; do
    printf '%s\0' 2 -d "$root/$prefix.XXXXXX"
  done > "$fixture/expected-allocation-args"
  cmp "$fixture/expected-allocation-args" "$allocation_args"
  allocated=()
  while IFS= read -r path; do
    allocated+=("$path")
  done < "$paths"
  # Cleanup removes all five paths, then revisits the registered graph home.
  {
    printf '%s\0' 6 -rf "${allocated[@]}"
    printf '%s\0' 3 -rf -- "${allocated[2]}"
  } > "$fixture/expected-cleanup-args"
  cmp "$fixture/expected-cleanup-args" "$cleanup_args"
  printf 'ok: mktemp and rm receive each spaced path as one argument on exit %s\n' "$expected_status"
  [[ "$(cat "$home_mode")" == 700 ]] || {
    printf 'temporary home was not private (0700) while in use\n' >&2; exit 1;
  }
  while IFS= read -r path; do
    [[ ! -e "$path" && ! -L "$path" ]]
  done < "$paths"
  [[ $(sort -u "$paths" | wc -l | tr -d ' ') == 5 ]]
  [[ -d "$root" && -f "$root/keep" ]]
  [[ $(find "$root" -mindepth 1 -maxdepth 1 | wc -l | tr -d ' ') == 1 ]]
  printf 'ok: distinct temporary paths honor spaced TMPDIR and are cleaned on exit %s\n' "$expected_status"
  printf 'ok: populated temporary home is private (0700) before exit %s\n' "$expected_status"
done

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

# Run the actual script offline and interrupt it at its first ready checkpoint,
# after LOCUS_HOME exists and the production cleanup traps have been installed.
(
  test_dir="$(mktemp -d "${TMPDIR:-/tmp}/locus-e2e-term-test.XXXXXX")"
  trap 'rm -rf -- "$test_dir"' EXIT
  mkdir "$test_dir/scratch with spaces"
  touch "$test_dir/scratch with spaces/keep"

  cat >"$test_dir/stubs.sh" <<'STUBS'
cargo() { return 0; }
printf() {
  if [[ $# -eq 2 && "$1" == '  ok  %s\n' && "$2" == 'isolated home ready' ]]; then
    [[ -d "$LOCUS_HOME" ]] || exit 74
    touch "$LOCUS_HOME/cleanup-sentinel"
    command printf '%s' "$LOCUS_HOME" >"$TEST_DIR/home-path"
    kill -TERM "$$"
    # If TERM fails to stop the script, fail without allowing later probes.
    exit 75
  fi
  command printf "$@"
}
STUBS
  cat >"$test_dir/app" <<'APP'
#!/usr/bin/env bash
touch "$TEST_DIR/app-called"
exit 76
APP
  chmod +x "$test_dir/app"

  status=0
  TEST_DIR="$test_dir" TMPDIR="$test_dir/scratch with spaces" \
    BASH_ENV="$test_dir/stubs.sh" LOCUS_CONTROL_CAPABILITY=synthetic \
    LOCUS_BIN="$test_dir/app" MCP_BIN="$test_dir/app" \
    bash "$script_dir/e2e.sh" >"$test_dir/stdout" 2>"$test_dir/stderr" || status=$?

  [[ $status -eq 143 ]] || { echo 'TERM did not exit with status 143' >&2; exit 1; }
  [[ -s "$test_dir/home-path" ]] || { echo 'TERM checkpoint was not reached' >&2; exit 1; }
  home="$(cat "$test_dir/home-path")"
  [[ "${home%/*}" == "$test_dir/scratch with spaces" && "${home##*/}" == locus-e2e.* ]]
  [[ ! -e "$home" && ! -L "$home" ]] || { echo 'home survived TERM cleanup' >&2; exit 1; }
  [[ -f "$test_dir/scratch with spaces/keep" ]]
  [[ $(find "$test_dir/scratch with spaces" -mindepth 1 -maxdepth 1 | wc -l | tr -d ' ') == 1 ]]
  [[ ! -e "$test_dir/app-called" ]] || { echo 'application ran before TERM' >&2; exit 1; }
)
printf 'ok: TERM removes the created home and exits nonzero\n'
