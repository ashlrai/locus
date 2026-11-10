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
for name in e2e_cleanup create_workspace; do
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
