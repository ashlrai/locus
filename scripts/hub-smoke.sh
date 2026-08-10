#!/usr/bin/env bash
# hub-smoke.sh — validate ashlr-hub CLI contracts against a throwaway LOCUS_HOME.
#
# Requires: locus (or cargo build), jq
# Safe: never touches ~/.locus; never prints secret values.
#
# Resilience notes (CI):
#   After heavy cargo tests, the first authority-broker spawn can race a tight
#   handoff window (debug binary cold start + executable identity hash + socket
#   bind). Core defaults to a 10s wait and honors
#   LOCUS_AUTHORITY_BROKER_START_TIMEOUT_MS; this script also retries init/pin
#   a few times so hub-smoke stays green under runner load. Webhook/export
#   code is not on this path — pin only needs a live local broker.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# Prefer freshly built binaries over an older cargo install.
export PATH="${ROOT}/target/debug:${ROOT}/target/release:${HOME}/.cargo/bin:${PATH}"
export LOCUS_CONTROL_CAPABILITY="${LOCUS_CONTROL_CAPABILITY:-$(od -An -N32 -tx1 /dev/urandom | tr -d ' \n')}"
# Give the supervised broker more headroom on contested CI runners (clamped in core).
export LOCUS_AUTHORITY_BROKER_START_TIMEOUT_MS="${LOCUS_AUTHORITY_BROKER_START_TIMEOUT_MS:-15000}"

if ! command -v jq >/dev/null 2>&1; then
  echo "error: jq is required" >&2
  exit 1
fi

# Always ensure a local debug build has `agent report`.
if ! locus agent report --help >/dev/null 2>&1; then
  echo "building locus with agent report…"
  (cd "$ROOT" && cargo build -q -p locus-cli)
fi
# Rebuild if target is stale relative to sources (best-effort).
if [[ ! -x "${ROOT}/target/debug/locus" ]]; then
  (cd "$ROOT" && cargo build -q -p locus-cli)
fi

SMOKE_HOME="$(mktemp -d "${TMPDIR:-/tmp}/locus-hub-smoke.XXXXXX")"
SMOKE_PROJ="$(mktemp -d "${TMPDIR:-/tmp}/locus-hub-proj.XXXXXX")"
SMOKE_ERR="$(mktemp "${TMPDIR:-/tmp}/locus-hub-smoke-err.XXXXXX")"
cleanup() { rm -rf "$SMOKE_HOME" "$SMOKE_PROJ" "$SMOKE_ERR"; }
trap cleanup EXIT

export LOCUS_HOME="$SMOKE_HOME"
unset LOCUS_SESSION_ID || true

echo "== hub-smoke LOCUS_HOME=$LOCUS_HOME =="
echo "locus: $(command -v locus) ($(locus --version 2>/dev/null || true))"

# Pre-fault the binary into page cache so the broker child is less likely to
# miss the handoff window on cold CI disks.
locus --help >/dev/null 2>&1 || true

# init + pin both need the live authority broker (require_local_control).
# Retry a few times; leave between attempts so a half-started broker can exit.

smoke_init_and_pin() {
  local attempt=1
  local max_attempts=3
  while (( attempt <= max_attempts )); do
    : >"$SMOKE_ERR"
    if locus init --with-samples >/dev/null 2>"$SMOKE_ERR" \
      && locus pin personal >/dev/null 2>"$SMOKE_ERR"; then
      return 0
    fi
    if (( attempt == max_attempts )); then
      echo "error: hub-smoke init/pin failed after ${max_attempts} attempts" >&2
      head -c 800 "$SMOKE_ERR" >&2 || true
      echo >&2
      # Surface a final un-silenced pin for CI logs.
      locus pin personal 2>&1 || true
      return 1
    fi
    echo "warn: hub-smoke init/pin attempt ${attempt} failed; retrying…" >&2
    head -c 400 "$SMOKE_ERR" >&2 || true
    echo >&2
    locus leave >/dev/null 2>&1 || true
    sleep "$attempt"
    attempt=$((attempt + 1))
  done
  return 1
}

smoke_init_and_pin

# Project dir with MCP registration so agent status can become ready
cd "$SMOKE_PROJ"
printf '%s\n' '{"mcpServers":{"locus":{"command":"locus-mcp","args":[]}}}' > .mcp.json

fail=0

check_json() {
  local name="$1"
  local cmd="$2"
  local jq_expr="$3"
  local out code=0
  set +e
  out="$(eval "$cmd" 2>/dev/null)"
  code=$?
  set -e
  # doctor / agent report may exit non-zero (WARN/protected) — still must emit valid JSON
  if ! printf '%s' "$out" | jq -e . >/dev/null 2>&1; then
    echo "FAIL  $name — not JSON (exit=$code)"
    echo "$out" | head -c 400
    fail=1
    return
  fi
  if ! printf '%s' "$out" | jq -e "$jq_expr" >/dev/null 2>&1; then
    echo "FAIL  $name — jq predicate failed: $jq_expr"
    printf '%s\n' "$out" | jq -c . | head -c 800
    echo
    fail=1
    return
  fi
  # Secret hygiene: refuse common token prefixes in JSON payload
  if printf '%s' "$out" | grep -EEq 'ghp_|sk-[a-zA-Z0-9]{10,}|xox[baprs]-'; then
    echo "FAIL  $name — possible secret material in output"
    fail=1
    return
  fi
  echo "ok    $name (exit=$code)"
}

check_json "agent report --json" \
  "locus agent report --json" \
  '(.status | IN("ready","protected","unsafe"))
   and (.ready | type) == "boolean"
   and (.exit_code | IN(0,1,2))
   and (.status_oneline | type) == "string"
   and .mcp_command == "locus-mcp"
   and (.required_servers | index("locus") != null)
   and (.required_servers | index("phantom") != null)
   and has("mcp_registered")
   and has("doctor")
   and has("commands")
   and has("home")
   and .doctor.home != null
   and (.doctor.verdict | IN("SAFE","WARN","UNSAFE"))'

check_json "doctor --json" \
  "locus doctor --json" \
  'has("version") and has("home") and has("seal_ok") and has("bindings")
   and has("runtime") and has("approvals") and has("pending_approvals")
   and has("dual_control_waiting") and has("phantom_on_path")
   and has("unresolved_phm") and has("autopin") and has("workspace")
   and has("audit") and has("findings") and has("issues")
   and has("verdict") and has("ok")
   and (.verdict | IN("SAFE","WARN","UNSAFE"))'

check_json "whoami --json" \
  "locus whoami --json" \
  'has("session_id") and has("binding_alias") and has("tenant")
   and has("providers") and has("seal_ok") and has("mode")'

check_json "status --json" \
  "locus status --json" \
  '.pinned == true and has("binding") and has("tenant") and has("session_id")'

oneline="$(locus status --oneline)"
if [[ "$oneline" != *":"* ]]; then
  echo "FAIL  status --oneline expected alias:tenant, got: $oneline"
  fail=1
else
  echo "ok    status --oneline ($oneline)"
fi

# Unpinned path still produces agent report JSON
locus leave >/dev/null 2>&1 || true
check_json "agent report unpinned" \
  "locus agent report --json" \
  '.status_oneline == "unpinned" and .ready == false'

# Schema files present
for f in agent-report.schema.json doctor.schema.json hub-gate.schema.json; do
  if [[ -f "$ROOT/schema/$f" ]]; then
    echo "ok    schema/$f present"
  else
    echo "FAIL  schema/$f missing"
    fail=1
  fi
done

# Docs present
for f in docs/hub-integration.md integrations/ashlr-hub/README.md integrations/ashlr-hub/fleet-preflight.md; do
  if [[ -f "$ROOT/$f" ]]; then
    echo "ok    $f present"
  else
    echo "FAIL  $f missing"
    fail=1
  fi
done

if [[ "$fail" -ne 0 ]]; then
  echo "== hub-smoke FAILED =="
  exit 1
fi
echo "== hub-smoke OK =="
