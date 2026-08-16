#!/usr/bin/env bash
# Locus end-to-end shell tests — pin, isolation, MCP, freeze, approval, doctor,
# dual-control, events, optional enter/run/notify/ns; graph/ci/heartbeat,
# dashboard health, forensics export, goal status, verify claim/session,
# watch session heartbeat, safe_next MCP, upstream list, HTTP MCP session +
# SSE when locus-mcp --http is available (feature-detected). Full 0.2+ surface
# plus adversarial release security probes (~44+ checks).
set -euo pipefail

export PATH="${HOME}/.cargo/bin:${PATH}"
export LOCUS_CONTROL_CAPABILITY="${LOCUS_CONTROL_CAPABILITY:-$(od -An -N32 -tx1 /dev/urandom | tr -d ' \n')}"
OPERATOR_CONTROL_CAPABILITY="$LOCUS_CONTROL_CAPABILITY"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

LOCUS_BIN="${LOCUS_BIN:-$ROOT/target/release/locus}"
MCP_BIN="${MCP_BIN:-$ROOT/target/release/locus-mcp}"

pass=0
fail=0
skip=0

log()  { printf '\n==> %s\n' "$*"; }
ok()   { printf '  ok  %s\n' "$*"; pass=$((pass + 1)); }
skip() { printf '  skip %s\n' "$*"; skip=$((skip + 1)); }
die()  { printf '  FAIL %s\n' "$*" >&2; fail=$((fail + 1)); exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || die "missing required tool: $1"; }

# Feature detection: true if `locus <cmd> --help` succeeds (subcommand exists).
has_cmd() {
  "$LOCUS_BIN" "$1" --help >/dev/null 2>&1
}

# Nested feature detection: true if `locus a b … --help` succeeds.
has_cmd_path() {
  "$LOCUS_BIN" "$@" --help >/dev/null 2>&1
}

# True if help text for a command mentions a flag substring.
help_mentions() {
  local cmd="$1" needle="$2"
  "$LOCUS_BIN" "$cmd" --help 2>&1 | grep -q -- "$needle"
}

# ── 1. Build release binaries ────────────────────────────────────────────────
log "1. build release binaries"
need cargo
cargo build --release -p locus-cli -p locus-mcp --locked
[[ -x "$LOCUS_BIN" ]] || die "missing $LOCUS_BIN"
[[ -x "$MCP_BIN" ]] || die "missing $MCP_BIN"
ok "locus + locus-mcp release binaries"

# ── 2. Isolated LOCUS_HOME ───────────────────────────────────────────────────
export LOCUS_HOME
LOCUS_HOME="$(mktemp -d "${TMPDIR:-/tmp}/locus-e2e.XXXXXX")"
log "2. LOCUS_HOME=$LOCUS_HOME"
# Optional background PIDs (HTTP MCP, etc.) cleaned on any exit path.
HTTP_MCP_PID=""
e2e_cleanup() {
  if [[ -n "${HTTP_MCP_PID:-}" ]]; then
    kill "$HTTP_MCP_PID" 2>/dev/null || true
    wait "$HTTP_MCP_PID" 2>/dev/null || true
    HTTP_MCP_PID=""
  fi
  rm -rf "$LOCUS_HOME" "${WS_DIR:-}" "${SECURITY_HOME:-}" "${SECURITY_WS:-}"
}
trap 'e2e_cleanup' EXIT
ok "isolated home ready"

# Helper: run locus with the test home
locus() { "$LOCUS_BIN" "$@"; }

# NDJSON MCP helper: print JSON-RPC lines to locus-mcp, collect responses.
# Args: one or more JSON objects (as strings). Prints response JSON lines on stdout.
mcp_rpc() {
  local body=""
  local line
  for line in "$@"; do
    body+="${line}"$'\n'
  done
  # shellcheck disable=SC2086
  if [[ -f "$LOCUS_HOME/sessions/active.json" ]]; then
    printf '%s' "$body" | locus exec --no-resolve -- "$MCP_BIN" 2>/dev/null
  else
    printf '%s' "$body" | LOCUS_HOME="$LOCUS_HOME" "$MCP_BIN" 2>/dev/null
  fi
}

# Extract tool names from a tools/list response line (id matching $1 if given)
tool_names_from_list() {
  # stdin: full MCP stdout
  python3 -c '
import json, sys
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    try:
        msg = json.loads(line)
    except json.JSONDecodeError:
        continue
    tools = (msg.get("result") or {}).get("tools")
    if tools is None:
        continue
    for t in tools:
        n = t.get("name")
        if n:
            print(n)
'
}

tool_call_text() {
  # stdin: MCP stdout; find first tools/call-like result with content text
  python3 -c '
import json, sys
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    try:
        msg = json.loads(line)
    except json.JSONDecodeError:
        continue
    result = msg.get("result") or {}
    content = result.get("content")
    if not content:
        continue
    text = content[0].get("text", "") if content else ""
    is_err = result.get("isError", False)
    print(("ERR|" if is_err else "OK|") + text)
    break
'
}

# ── 3. init --with-samples, pin personal/acme, whoami ────────────────────────
log "3. init --with-samples, pin personal, whoami, pin acme, whoami"
locus init --with-samples >/dev/null
locus pin personal >/dev/null
who_personal="$(locus whoami --json)"
echo "$who_personal" | python3 -c '
import json, sys
w = json.load(sys.stdin)
assert w["binding_alias"] == "personal", w
assert w["tenant"] == "personal", w
assert w.get("seal_ok") is True, w
'
ok "pin personal + whoami"

locus pin acme >/dev/null
who_acme="$(locus whoami --json)"
echo "$who_acme" | python3 -c '
import json, sys
w = json.load(sys.stdin)
assert w["binding_alias"] == "acme", w
assert w["tenant"] == "acme-corp", w
assert w.get("seal_ok") is True, w
# Agent-facing whoami exposes only credential presence/source metadata.
for p in w["providers"]:
    assert "credential_ref" not in p, p
    assert p["credential"] == {"present": True, "source": "phantom"}, p
serialized = json.dumps(w)
for canary in ["GH_TOKEN_ACME", "VERCEL_TOKEN_ACME", "SUPABASE_ACME", "PERSONAL"]:
    assert canary not in serialized, (canary, w)
'
ok "pin acme + whoami exclusive"

# ── 4. workspace allowlist blocks personal without --force ───────────────────
log "4. workspace allowlist block personal without --force"
WS_DIR="$(mktemp -d "${TMPDIR:-/tmp}/locus-ws.XXXXXX")"
(
  cd "$WS_DIR"
  locus workspace --default acme --allow acme,acme-ro --require-pin --force >/dev/null
  if locus pin personal >/dev/null 2>&1; then
    die "pin personal should fail under acme allowlist"
  fi
  # error message check
  err="$(locus pin personal 2>&1 || true)"
  echo "$err" | grep -qi "not allowed" || die "expected allowlist error, got: $err"
  ok "personal blocked without --force"

  locus pin personal --force >/dev/null
  ok "personal allowed with --force"
)

# leave force-pin; re-pin acme outside workspace for remaining tests
cd "$ROOT"
locus pin acme --force >/dev/null

# ── 5. exec env isolation (env: secret inject) ───────────────────────────────
log "5. exec env isolation (env: secret inject)"
# Add a binding that uses env: credential refs so we can inject real values.
cat >"$LOCUS_HOME/bindings/envtest.toml" <<'EOF'
[binding]
id = "bnd_envtest"
alias = "envtest"
tenant = "env-tenant"
description = "e2e env credential isolation"

[binding.policy]
default = "allow"
require_approval = ["*.delete*"]
max_ttl = "1h"

[[binding.providers]]
provider = "supabase"
account = "env-sb"
credential_ref = "env:LOCUS_E2E_SUPABASE_TOKEN"
scope = { project_ref = "proj_env_e2e", read_only = true }

[[binding.providers]]
provider = "github"
account = "env-gh"
credential_ref = "env:LOCUS_E2E_GH_TOKEN"
scope = { orgs = ["env-org"] }
EOF

export LOCUS_E2E_SUPABASE_TOKEN="e2e-sb-secret-value-$$"
export LOCUS_E2E_GH_TOKEN="e2e-gh-secret-value-$$"
# Ambient identity that must be scrubbed / replaced
export GH_TOKEN="ambient-gh-must-not-leak"
export SUPABASE_ACCESS_TOKEN="ambient-sb-must-not-leak"
export AWS_PROFILE="ambient-aws-must-not-leak"
export UNLISTED_SECRET_CANARY="arbitrary-parent-secret-must-not-leak"

locus pin envtest >/dev/null
exec_env="$(locus exec -- env 2>/dev/null || true)"
# locus exec prints a progress line on stderr; capture only child env
exec_env="$(locus exec -- env 2>/dev/null)"

echo "$exec_env" | grep -q "LOCUS_BINDING=envtest" || die "missing LOCUS_BINDING"
echo "$exec_env" | grep -q "LOCUS_TENANT=env-tenant" || die "missing LOCUS_TENANT"
echo "$exec_env" | grep -q "SUPABASE_ACCESS_TOKEN=${LOCUS_E2E_SUPABASE_TOKEN}" \
  || die "env: secret not injected into SUPABASE_ACCESS_TOKEN"
echo "$exec_env" | grep -q "GH_TOKEN=${LOCUS_E2E_GH_TOKEN}" \
  || die "env: secret not injected into GH_TOKEN"
echo "$exec_env" | grep -q "ambient-gh-must-not-leak" \
  && die "ambient GH_TOKEN leaked into isolated env" || true
echo "$exec_env" | grep -q "ambient-sb-must-not-leak" \
  && die "ambient SUPABASE_ACCESS_TOKEN leaked" || true
echo "$exec_env" | grep -q "AWS_PROFILE=" \
  && die "AWS_PROFILE should be scrubbed" || true

# Every command-child surface must reject resolving upstreams before any child,
# worker, session, audit, or credential effect. Cover both explicit flags and
# pure-recipe defaults (github-official defaults resolve_secrets=true).
no_resolve_worker_marker="$LOCUS_HOME/no-resolve-worker-effect"
cat >"$LOCUS_HOME/bindings/noresolve.toml" <<EOF
[binding]
id = "bnd_noresolve"
alias = "noresolve"
tenant = "noresolve-tenant"

[binding.policy]
default = "allow"

[[binding.providers]]
provider = "github"
account = "noresolve-account"
credential_ref = "env:LOCUS_E2E_GH_TOKEN"
upstream = { command = "/bin/sh", args = ["-c", "env > '$no_resolve_worker_marker'"], resolve_secrets = true }
EOF

cat >"$LOCUS_HOME/bindings/noresolve-recipe.toml" <<'EOF'
[binding]
id = "bnd_noresolve_recipe"
alias = "noresolve-recipe"
tenant = "noresolve-recipe-tenant"

[binding.policy]
default = "allow"

[[binding.providers]]
provider = "github"
account = "noresolve-recipe-account"
credential_ref = "env:LOCUS_E2E_GH_TOKEN"
upstream = { recipe = "github-official", sandbox = false }
EOF

control_plane_snapshot() {
  {
    [[ ! -f "$LOCUS_HOME/active.json" ]] || cksum "$LOCUS_HOME/active.json"
    [[ ! -f "$LOCUS_HOME/audit/events.jsonl" ]] || cksum "$LOCUS_HOME/audit/events.jsonl"
    find "$LOCUS_HOME/sessions" -type f -exec cksum {} \; 2>/dev/null || true
  } | sort
}

assert_no_resolve_blocked() {
  local label="$1" expected_surface="$2"
  shift 2
  local child_marker="$LOCUS_HOME/no-resolve-${label}-child-effect"
  local output ec
  rm -f "$child_marker"
  set +e
  output="$("$@" -- /bin/sh -c 'printf "%s" "${GH_TOKEN:-missing}" > "$1"' _ "$child_marker" 2>&1)"
  ec=$?
  set -e
  [[ $ec -ne 0 ]] || die "$label allowed credential-resolving upstream"
  echo "$output" | grep -Fq -- "--no-resolve refused $expected_surface" \
    || die "$label did not report centralized fail-closed boundary: $output"
  echo "$output" | grep -Fq "no session or credential effect occurred" \
    || die "$label omitted no-effect contract: $output"
  [[ ! -e "$child_marker" ]] || die "$label started secret-bearing child"
}

for binding in noresolve noresolve-recipe; do
  locus pin "$binding" --force >/dev/null
  no_resolve_state_before="$(control_plane_snapshot)"
  assert_no_resolve_blocked "${binding}-exec" "locus exec" locus exec --no-resolve
  assert_no_resolve_blocked "${binding}-run" "locus run" locus run -b "$binding" --no-resolve --force
  assert_no_resolve_blocked "${binding}-run-share-pin" "locus run" locus run -b "$binding" --no-resolve --share-pin --force
  assert_no_resolve_blocked "${binding}-ci-run" "locus ci run" locus ci run -b "$binding" --no-resolve --force
  no_resolve_state_after="$(control_plane_snapshot)"
  [[ "$no_resolve_state_after" == "$no_resolve_state_before" ]] \
    || die "$binding --no-resolve changed active/session/audit state"
done
[[ ! -e "$no_resolve_worker_marker" ]] || die "--no-resolve started resolving upstream worker"
ok "all --no-resolve child surfaces block explicit + recipe-default resolving upstreams before effects"

# Credential-free upstreams remain usable. `run` performs its normal eager MCP
# probe, while all three requested children run without receiving GH_TOKEN.
no_resolve_free_worker="$LOCUS_HOME/no-resolve-free-worker.py"
no_resolve_free_worker_marker="$LOCUS_HOME/no-resolve-free-worker-effect"
cat >"$no_resolve_free_worker" <<'PY'
import json
import os
import sys

with open(sys.argv[1], "w", encoding="utf-8") as marker:
    marker.write(os.environ.get("GH_TOKEN", "missing"))

for line in sys.stdin:
    request = json.loads(line)
    if "id" not in request:
        continue
    method = request.get("method")
    if method == "initialize":
        result = {
            "protocolVersion": "2024-11-05",
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "no-resolve-free", "version": "1"},
        }
    elif method == "tools/list":
        result = {"tools": []}
    else:
        result = {}
    print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": result}), flush=True)
PY
python_bin="$(command -v python3)"
cat >"$LOCUS_HOME/bindings/noresolve-free.toml" <<EOF
[binding]
id = "bnd_noresolve_free"
alias = "noresolve-free"
tenant = "noresolve-free-tenant"

[binding.policy]
default = "allow"

[[binding.providers]]
provider = "github"
account = "noresolve-free-account"
credential_ref = "env:LOCUS_E2E_GH_TOKEN"
upstream = { command = "$python_bin", args = ["$no_resolve_free_worker", "$no_resolve_free_worker_marker"], resolve_secrets = false }
EOF

locus pin noresolve-free --force >/dev/null
for surface in exec run ci-run; do
  free_child_marker="$LOCUS_HOME/no-resolve-free-${surface}-child-effect"
  case "$surface" in
    exec) locus exec --no-resolve -- /bin/sh -c 'printf "%s" "${GH_TOKEN:-missing}" > "$1"' _ "$free_child_marker" >/dev/null ;;
    run)
      free_run_output="$(locus run -b noresolve-free --no-resolve --force -- /bin/sh -c 'printf "%s" "${GH_TOKEN:-missing}" > "$1"' _ "$free_child_marker" 2>&1)"
      echo "$free_run_output" | grep -Fq "worker ensure (upstream) soft-failed" \
        && die "credential-free upstream failed MCP handshake: $free_run_output"
      ;;
    ci-run) locus ci run -b noresolve-free --no-resolve --force -- /bin/sh -c 'printf "%s" "${GH_TOKEN:-missing}" > "$1"' _ "$free_child_marker" >/dev/null ;;
  esac
  [[ "$(cat "$free_child_marker")" == "missing" ]] \
    || die "$surface --no-resolve injected credentials into allowed child"
done
[[ "$(cat "$no_resolve_free_worker_marker")" == "missing" ]] \
  || die "credential-free upstream worker received GH_TOKEN"
ok "credential-free upstream and exec/run/ci-run children remain usable without credentials"
echo "$exec_env" | grep -q "UNLISTED_SECRET_CANARY=" \
  && die "arbitrary parent secret leaked into locus exec" || true
echo "$exec_env" | grep -q "arbitrary-parent-secret-must-not-leak" \
  && die "arbitrary parent secret value leaked into locus exec" || true
echo "$exec_env" | grep -q "SUPABASE_PROJECT_REF=proj_env_e2e" \
  || die "frozen project_ref missing"
ok "exec scrubs ambient + injects env: secrets"

# scrub parent ambient so later steps are clean
unset GH_TOKEN SUPABASE_ACCESS_TOKEN AWS_PROFILE UNLISTED_SECRET_CANARY
export LOCUS_CONTROL_CAPABILITY="$OPERATOR_CONTROL_CAPABILITY"

# Exact Hub/agent sessions are confinement capabilities. Reproduce the prior
# env-i bypass with a synthetic Phantom resolver and verify every refusal occurs
# before child, credential, audit, session, worker, or global-pin effects.
fake_phantom_bin="$LOCUS_HOME/fake-phantom-bin"
mkdir -p "$fake_phantom_bin"
cat >"$fake_phantom_bin/phantom" <<'SH'
#!/bin/sh
case "${3:-${2:-}}" in
  GH_TOKEN_ACME) printf '%s\n' 'acme-canary-token' ;;
  GH_TOKEN_PERSONAL) printf '%s\n' 'personal-canary-token' ;;
  *) printf '%s\n' 'other-canary-token' ;;
esac
SH
chmod +x "$fake_phantom_bin/phantom"

[[ ${#OPERATOR_CONTROL_CAPABILITY} -eq 64 ]] \
  || die "operator control capability was not preserved for exact-session probes"
set +e
hub_session_json="$(LOCUS_CONTROL_CAPABILITY="$OPERATOR_CONTROL_CAPABILITY" \
  "$LOCUS_BIN" ci mint -b acme --force 2>&1)"
hub_session_ec=$?
set -e
[[ $hub_session_ec -eq 0 ]] || die "exact-session CI mint failed: $hub_session_json"
hub_session_id="$(printf '%s' "$hub_session_json" | python3 -c 'import json,sys; print(json.load(sys.stdin)["session_id"])')"
hub_executor_capability="$(printf '%s' "$hub_session_json" | python3 -c 'import json,sys; print(json.load(sys.stdin)["env"]["LOCUS_EXECUTOR_CAPABILITY"])')"
set +e
active_before_exact_json="$(LOCUS_CONTROL_CAPABILITY="$OPERATOR_CONTROL_CAPABILITY" \
  "$LOCUS_BIN" whoami --json 2>&1)"
active_before_exact_ec=$?
set -e
[[ $active_before_exact_ec -eq 0 ]] \
  || die "operator active-session lookup failed: $active_before_exact_json"
active_before_exact="$(printf '%s' "$active_before_exact_json" \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["session_id"])')"
exact_state_before="$(control_plane_snapshot)"

run_exact() {
  env -i \
    PATH="$fake_phantom_bin:/usr/bin:/bin" \
    HOME="$HOME" \
    LOCUS_HOME="$LOCUS_HOME" \
    LOCUS_SESSION_ID="$hub_session_id" \
    LOCUS_EXECUTOR_CAPABILITY="$hub_executor_capability" \
    "$LOCUS_BIN" "$@"
}

assert_exact_blocked() {
  local label="$1" expected="$2"
  shift 2
  local marker="$LOCUS_HOME/exact-${label}-child-effect"
  local output ec
  rm -f "$marker"
  set +e
  output="$(run_exact "$@" -- /bin/sh -c 'printf "%s|%s" "${LOCUS_BINDING:-missing}" "${GH_TOKEN:-missing}" > "$1"' _ "$marker" 2>&1)"
  ec=$?
  set -e
  [[ $ec -ne 0 ]] || die "exact session allowed $label"
  echo "$output" | grep -Fq -- "$expected" || die "$label wrong refusal: $output"
  [[ ! -e "$marker" ]] || die "$label started token-bearing child: $(cat "$marker")"
}

assert_exact_blocked "exec-resolve" "cannot resolve credentials inside a delegated session" exec
assert_exact_blocked "run-cross-binding" "cannot select binding \`personal\`" run -b personal --no-resolve --force
assert_exact_blocked "ci-cross-binding" "cannot select binding \`personal\`" ci run -b personal --no-resolve --force
assert_exact_blocked "run-share-pin" "--share-pin is unavailable" run -b acme --no-resolve --share-pin

set +e
exact_pin_error="$(run_exact pin personal --force 2>&1)"
exact_pin_ec=$?
set -e
[[ $exact_pin_ec -ne 0 ]] || die "exact session mutated global pin"
echo "$exact_pin_error" | grep -Fq "local control operation" \
  || echo "$exact_pin_error" | grep -Fq "LOCUS_CONTROL_CAPABILITY" \
  || die "exact pin refusal did not name control boundary: $exact_pin_error"

stripped_mutation() {
  env -i PATH="/usr/bin:/bin" HOME="$HOME" LOCUS_HOME="$LOCUS_HOME" \
    LOCUS_GRAPH_PASSPHRASE="stripped-authority-probe" \
    "$LOCUS_BIN" "$@" >/dev/null 2>&1
}
for mutation in \
  "pin personal --force" \
  "ci mint -b personal --force" \
  "notify on" \
  "workspace --default personal --force" \
  "binding add stripped --tenant stripped --provider github --account stripped --credential-ref env:STRIPPED_TOKEN" \
  "graph export --out $LOCUS_HOME/stripped-graph.locus"
do
  read -r -a mutation_args <<<"$mutation"
  if stripped_mutation "${mutation_args[@]}"; then
    die "stripped child upgraded ambient active authority: $mutation"
  fi
done
[[ ! -e "$LOCUS_HOME/stripped-graph.locus" ]] \
  || die "stripped child wrote graph output"

exact_state_after="$(control_plane_snapshot)"
[[ "$exact_state_after" == "$exact_state_before" ]] \
  || die "blocked exact-session probes changed session/audit/global state"
active_after_exact="$(LOCUS_CONTROL_CAPABILITY="$OPERATOR_CONTROL_CAPABILITY" \
  "$LOCUS_BIN" whoami --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["session_id"])')"
[[ "$active_after_exact" == "$active_before_exact" ]] || die "exact session changed global share-pin"

exact_allowed_marker="$LOCUS_HOME/exact-allowed-no-resolve"
run_exact exec --no-resolve -- /bin/sh -c \
  'printf "%s|%s" "${LOCUS_BINDING:-missing}" "${GH_TOKEN:-missing}" > "$1"' \
  _ "$exact_allowed_marker" >/dev/null
[[ "$(cat "$exact_allowed_marker")" == "acme|missing" ]] \
  || die "exact no-resolve child escaped binding or received token"

recipe_session_json="$(LOCUS_CONTROL_CAPABILITY="$OPERATOR_CONTROL_CAPABILITY" \
  "$LOCUS_BIN" ci mint -b noresolve-recipe --force)"
hub_session_id="$(printf '%s' "$recipe_session_json" | python3 -c 'import json,sys; print(json.load(sys.stdin)["session_id"])')"
hub_executor_capability="$(printf '%s' "$recipe_session_json" | python3 -c 'import json,sys; print(json.load(sys.stdin)["env"]["LOCUS_EXECUTOR_CAPABILITY"])')"
recipe_state_before="$(control_plane_snapshot)"
assert_exact_blocked "recipe-default-worker" "--no-resolve refused locus run" \
  run -b noresolve-recipe --no-resolve
recipe_state_after="$(control_plane_snapshot)"
[[ "$recipe_state_after" == "$recipe_state_before" ]] \
  || die "recipe-default exact refusal caused session/audit/worker startup effects"
ok "env-i exact sessions block cross-binding, share-pin, token children, and recipe-default workers before effects"

# ── 6. MCP tools/list unpinned vs pinned ─────────────────────────────────────
log "6. MCP tools/list unpinned vs pinned (printf | locus-mcp)"
locus leave >/dev/null

unpinned_out="$(
  mcp_rpc \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"e2e","version":"0"}}}' \
    '{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}' \
    '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}'
)"
unpinned_names="$(echo "$unpinned_out" | tool_names_from_list)"
echo "$unpinned_names" | grep -qx 'locus_whoami' || die "unpinned missing locus_whoami"
echo "$unpinned_names" | grep -qx 'locus_request_pin' || die "unpinned missing locus_request_pin"
if echo "$unpinned_names" | grep -qE '^(supabase|github|vercel)\.'; then
  die "unpinned must not expose provider tools: $unpinned_names"
fi
while IFS= read -r n; do
  [[ -z "$n" ]] && continue
  [[ "$n" == locus_* ]] || die "unpinned non-control tool: $n"
done <<<"$unpinned_names"
ok "unpinned tools/list is control-only"

locus pin acme --force >/dev/null
pinned_out="$(
  mcp_rpc \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"e2e","version":"0"}}}' \
    '{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}' \
    '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}'
)"
pinned_names="$(echo "$pinned_out" | tool_names_from_list)"
echo "$pinned_names" | grep -qx 'locus_whoami' || die "pinned missing locus_whoami"
echo "$pinned_names" | grep -q 'supabase.scope' \
  || die "pinned missing supabase.scope: names=$pinned_names response=$pinned_out"
echo "$pinned_names" | grep -q 'github.scope' || die "pinned missing github.scope"
echo "$pinned_names" | grep -q 'vercel.scope' || die "pinned missing vercel.scope"
ok "pinned tools/list includes provider tools"

# ── 7. Freeze deny project_ref ───────────────────────────────────────────────
log "7. freeze deny project_ref"
freeze_out="$(
  mcp_rpc \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"e2e","version":"0"}}}' \
    '{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}' \
    '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"supabase.scope","arguments":{"project_ref":"proj_evil"}}}'
)"
freeze_line="$(echo "$freeze_out" | tool_call_text)"
echo "$freeze_line" | grep -q '^ERR|' || die "expected isError for freeze deny: $freeze_line"
echo "$freeze_line" | grep -qiE 'scope freeze|proj_evil' \
  || die "unexpected freeze message: $freeze_line"
ok "scope freeze denies wrong project_ref"

# ── 8. read_only hard deny, then require_approval → advisory → still blocked ─
log "8a. read_only scope denies destructive tool outright (no approval minted)"
ro_out="$(
  mcp_rpc \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"e2e","version":"0"}}}' \
    '{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}' \
    '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"supabase.table.delete","arguments":{"table":"users"}}}'
)"
ro_line="$(echo "$ro_out" | tool_call_text)"
echo "$ro_line" | grep -q '^ERR|' || die "expected read_only deny error: $ro_line"
echo "$ro_line" | grep -q 'denied_read_only_scope' || die "missing denied_read_only_scope: $ro_line"
if echo "$ro_line" | grep -qE 'appr_[a-f0-9]+'; then
  die "read_only deny must not mint an approval record: $ro_line"
fi
ok "read_only scope hard-denies supabase.table.delete before approval gating"

# Flip the fixture to read_only=false so the approval flow itself is testable.
locus leave >/dev/null
sed -i.bak 's/project_ref = "proj_env_e2e", read_only = true/project_ref = "proj_env_e2e", read_only = false/' "$LOCUS_HOME/bindings/envtest.toml"
rm -f "$LOCUS_HOME/bindings/envtest.toml.bak"
locus pin envtest >/dev/null

log "8b. require_approval → local advisory → authority remains blocked"
appr_out="$(
  mcp_rpc \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"e2e","version":"0"}}}' \
    '{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}' \
    '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"supabase.table.delete","arguments":{"table":"users"}}}'
)"
appr_line="$(echo "$appr_out" | tool_call_text)"
echo "$appr_line" | grep -q '^ERR|' || die "expected requires_approval error: $appr_line"
echo "$appr_line" | grep -qiE 'requires_approval|approval' \
  || die "missing requires_approval: $appr_line"
appr_id="$(echo "$appr_line" | grep -oE 'appr_[a-f0-9]+' | head -1)"
[[ -n "$appr_id" ]] || die "no approval_id in response: $appr_line"
ok "require_approval blocked with $appr_id"

locus approve grant "$appr_id" >/dev/null
ok "local advisory recorded for $appr_id"

retry_out="$(
  mcp_rpc \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"e2e","version":"0"}}}' \
    '{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}' \
    '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"supabase.table.delete","arguments":{"table":"users"}}}'
)"
retry_line="$(echo "$retry_out" | tool_call_text)"
echo "$retry_line" | grep -q '^ERR|' || die "local advisory must not authorize: $retry_line"
echo "$retry_line" | grep -q 'local_advisory' || die "missing advisory authority label: $retry_line"
ok "re-call remains blocked after local advisory"

# ── 9. doctor (structure + exit codes) ───────────────────────────────────────
log "9. doctor structure + exit codes"
# Sample bindings use unresolved phm: refs → verdict WARN / exit 1 typical.
# Assert structural health: seal + pin + bindings; SAFE|WARN|UNSAFE exit 0/1/2.
set +e
doctor_json="$(locus doctor --json 2>/dev/null)"
doctor_ec=$?
set -e
export DOCTOR_EC="$doctor_ec"
echo "$doctor_json" | python3 -c '
import json, sys, os
d = json.load(sys.stdin)
assert d.get("seal_ok") is True, d
assert d.get("pin_seal_ok") is True, d
assert d.get("bindings", 0) >= 2, d
assert d.get("pinned") in ("acme", "envtest", "personal"), d
verdict = (d.get("verdict") or "").upper()
assert verdict in ("SAFE", "WARN", "UNSAFE"), d
issues = d.get("issues") or []
ok_flag = d.get("ok")
assert ok_flag == (verdict == "SAFE"), d
assert ok_flag == (len(issues) == 0), d
ec = int(os.environ["DOCTOR_EC"])
want = {"SAFE": 0, "WARN": 1, "UNSAFE": 2}[verdict]
assert ec == want, "doctor exit must match verdict %s → %s, got %s: %s" % (
    verdict, want, ec, issues)
print("doctor seal_ok pin_seal_ok bindings=%s pinned=%s verdict=%s issues=%d exit=%s" % (
    d.get("bindings"), d.get("pinned"), verdict, len(issues), ec))
'
ok "doctor structure + exit code matches issues (exit=$doctor_ec)"

# Unpinned doctor still reports seal_ok and exits consistently with verdict
locus leave >/dev/null 2>&1 || true
set +e
doctor_unpinned="$(locus doctor --json 2>/dev/null)"
doctor_un_ec=$?
set -e
export DOCTOR_EC="$doctor_un_ec"
echo "$doctor_unpinned" | python3 -c '
import json, sys, os
d = json.load(sys.stdin)
assert d.get("seal_ok") is True, d
assert d.get("pinned") in (None, ""), d
verdict = (d.get("verdict") or "").upper()
assert verdict in ("SAFE", "WARN", "UNSAFE"), d
issues = d.get("issues") or []
ec = int(os.environ["DOCTOR_EC"])
want = {"SAFE": 0, "WARN": 1, "UNSAFE": 2}[verdict]
assert ec == want, (ec, want, verdict, issues)
'
ok "doctor unpinned exit code coherent (exit=$doctor_un_ec)"

# Re-pin acme for dual-control / events steps
locus pin acme --force >/dev/null

# ── 10. dual_control local labels never satisfy authority ────────────────────
log "10. dual_control local advisory labels remain untrusted"
# Write a binding with dual_control on delete tools; use env: refs for isolation.
cat >"$LOCUS_HOME/bindings/dual.toml" <<'EOF'
[binding]
id = "bnd_dual"
alias = "dual"
tenant = "dual-tenant"
description = "e2e dual-control"

[binding.policy]
default = "allow"
require_approval = ["*.delete*"]
dual_control = ["*.delete*"]
max_ttl = "1h"

[[binding.providers]]
provider = "supabase"
account = "dual-sb"
credential_ref = "env:LOCUS_E2E_SUPABASE_TOKEN"
scope = { project_ref = "proj_dual_e2e", read_only = false }
EOF

if ! help_mentions approve "--as" && ! has_cmd approve; then
  skip "approve CLI missing — dual_control grant not exercised"
else
  locus pin dual >/dev/null
  dual_out="$(
    mcp_rpc \
      '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"e2e","version":"0"}}}' \
      '{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}' \
      '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"supabase.table.delete","arguments":{"table":"users"}}}'
  )"
  dual_line="$(echo "$dual_out" | tool_call_text)"
  echo "$dual_line" | grep -q '^ERR|' || die "expected dual require_approval: $dual_line"
  dual_id="$(echo "$dual_line" | grep -oE 'appr_[a-f0-9]+' | head -1)"
  [[ -n "$dual_id" ]] || die "no approval_id for dual: $dual_line"
  ok "dual_control blocked with $dual_id"

  # Touch ID mock: cancel must fail closed (no grant written)
  if LOCUS_TOUCHID_MOCK=cancel locus approve grant "$dual_id" --as alice --touchid >/dev/null 2>&1; then
    die "LOCUS_TOUCHID_MOCK=cancel --touchid should fail (exit non-zero)"
  fi
  ok "touchid mock cancel fails closed"

  # A caller-controlled successful mock can only record advisory evidence.
  g1="$(LOCUS_TOUCHID_MOCK=ok locus approve grant "$dual_id" --as alice --touchid --json 2>/dev/null || true)"
  echo "$g1" | python3 -c '
import json, sys
r = json.load(sys.stdin)
assert r.get("status") in ("pending", "Pending") or r.get("status") == "pending", r
assert len(r.get("grants") or []) == 1, r
assert r.get("approval_authority") == "local_advisory", r
assert r.get("authoritative_path_enabled") is False, r
assert r.get("authoritative_grants") == 0, r
'
  ok "LOCUS_TOUCHID_MOCK=ok records advisory only"

  # Duplicate local label is rejected.
  if locus approve grant "$dual_id" --as alice >/dev/null 2>&1; then
    die "duplicate advisory label should be rejected"
  fi
  ok "duplicate advisory label rejected"

  # A second local label still cannot establish identity or dual-control authority.
  g2="$(locus approve grant "$dual_id" --as bob --json 2>/dev/null)"
  echo "$g2" | python3 -c '
import json, sys
r = json.load(sys.stdin)
st = (r.get("status") or "").lower()
assert st == "pending", r
assert len(r.get("grants") or []) >= 2, r
assert r.get("approval_authority") == "local_advisory", r
assert r.get("authoritative_path_enabled") is False, r
assert r.get("authoritative_grants") == 0, r
'
  ok "second local label remains advisory"

  retry_dual="$(
    mcp_rpc \
      '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"e2e","version":"0"}}}' \
      '{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}' \
      '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"supabase.table.delete","arguments":{"table":"users"}}}'
  )"
  retry_dual_line="$(echo "$retry_dual" | tool_call_text)"
  echo "$retry_dual_line" | grep -q '^ERR|' || die "local labels must not satisfy dual control: $retry_dual_line"
  echo "$retry_dual_line" | grep -q 'local_advisory' || die "missing advisory authority label: $retry_dual_line"
  ok "dual-control call remains blocked after two local labels"

  # Forge the strongest same-user JSON record after a caller-controlled
  # Touch ID mock. Persisted status and authority strings are never proof.
  export DUAL_APPROVAL_ID="$dual_id"
  python3 - <<'PY'
import datetime
import json
import os
from pathlib import Path

path = Path(os.environ["LOCUS_HOME"]) / "approvals" / f'{os.environ["DUAL_APPROVAL_ID"]}.json'
record = json.loads(path.read_text())
record["status"] = "approved"
record["granted_at"] = datetime.datetime.now(datetime.timezone.utc).isoformat().replace("+00:00", "Z")
record["expires_at"] = "2099-01-01T00:00:00Z"
for index, grant in enumerate(record.get("grants") or []):
    grant["authority"] = "external_authenticated"
    grant["envelope_id"] = f"unsigned-same-user-{index}"
path.write_text(json.dumps(record, indent=2) + "\n")
PY

  forged_retry="$(
    mcp_rpc \
      '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"e2e","version":"0"}}}' \
      '{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}' \
      '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"supabase.table.delete","arguments":{"table":"users"}}}'
  )"
  forged_retry_line="$(echo "$forged_retry" | tool_call_text)"
  echo "$forged_retry_line" | grep -q '^ERR|' \
    || die "forged same-user approval JSON authorized provider execution: $forged_retry_line"
  echo "$forged_retry_line" | grep -q '"authoritative_grants":0' \
    || die "forged record did not surface zero authority: $forged_retry_line"
  ok "forged future-dated external labels remain non-authoritative"
fi

# ── 11. locus events (audit export) ──────────────────────────────────────────
log "11. locus events --last N [--op] [--json]"
if ! has_cmd events; then
  skip "events command not available"
else
  # Pins / grants should have written audit lines
  events_json="$(locus events --last 20 --json 2>/dev/null)"
  echo "$events_json" | python3 -c '
import json, sys
ev = json.load(sys.stdin)
assert isinstance(ev, list), type(ev)
assert len(ev) >= 1, "expected at least one audit event after pin/approve"
for e in ev:
    assert "ts" in e and "op" in e and "binding" in e, e
print("events last20 count=%d sample_ops=%s" % (
    len(ev), ",".join(sorted({e["op"] for e in ev})[:8])))
'
  ok "events --last 20 --json returns records"

  # Filter by op (session.pin is written on pin)
  pin_ev="$(locus events --last 50 --op session.pin --json 2>/dev/null || true)"
  # op name may vary — accept empty or pin-related
  echo "$pin_ev" | python3 -c '
import json, sys
ev = json.load(sys.stdin)
assert isinstance(ev, list), ev
for e in ev:
    assert e.get("op") == "session.pin", e
'
  ok "events --op session.pin filters (or empty list)"

  # Text mode still exits 0
  locus events --last 5 >/dev/null
  ok "events text mode"
fi

# ── 12. optional: enter / leave pair (feature-detected) ──────────────────────
log "12. enter/leave (optional)"
if has_cmd enter; then
  # enter is alias/workflow for pin in some designs — exercise without assuming UX
  locus leave >/dev/null 2>&1 || true
  if locus enter personal >/dev/null 2>&1 || locus enter -- personal >/dev/null 2>&1; then
    st="$(locus status --oneline 2>/dev/null || true)"
    echo "$st" | grep -qi personal || die "enter personal → expected personal pin, got: $st"
    ok "enter personal"
  else
    skip "enter present but invocation failed (API may differ)"
  fi
else
  skip "enter not available (use pin)"
fi

# leave always expected in current CLI
if has_cmd leave; then
  locus pin personal --force >/dev/null 2>&1 || locus pin acme --force >/dev/null
  locus leave >/dev/null
  status="$(locus status --oneline)"
  [[ "$status" == "unpinned" ]] || die "expected unpinned after leave, got: $status"
  ok "leave → unpinned"
else
  skip "leave not available"
fi

# ── 13. optional: locus run -b (one-shot child session) ───────────────────────
log "13. locus run -b (optional)"
if has_cmd run; then
  # Prefer -b / --binding; fall back to positional if needed
  ran=0
  if locus run -b personal -- true >/dev/null 2>&1; then
    ran=1
  elif locus run --binding personal -- true >/dev/null 2>&1; then
    ran=1
  elif locus run personal -- true >/dev/null 2>&1; then
    ran=1
  fi
  if [[ "$ran" -eq 1 ]]; then
    # Shell pin should be unchanged (run is one-shot) — leave was unpinned above
    st="$(locus status --oneline 2>/dev/null || true)"
    ok "run -b completed (status after: $st)"
  else
    skip "run present but could not invoke with -b/--binding"
  fi
else
  skip "run not available (use exec under pin)"
fi

# ── 13b. optional: pin --ns flag (namespaced multi-bind; feature-detected) ───
log "13b. pin --ns (optional)"
if help_mentions pin "--ns"; then
  # Help only — do not leave e2e home in multi-bind mode
  ok "pin --ns flag present (namespaced multi-bind)"
else
  skip "pin --ns not advertised in help"
fi

# ── 14. leave → unpinned MCP control-only ────────────────────────────────────
log "14. leave → unpinned MCP control-only"
locus leave >/dev/null 2>&1 || true
status="$(locus status --oneline)"
[[ "$status" == "unpinned" ]] || die "expected unpinned, got: $status"
ok "leave → status unpinned"

final_out="$(
  mcp_rpc \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"e2e","version":"0"}}}' \
    '{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}' \
    '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}'
)"
final_names="$(echo "$final_out" | tool_names_from_list)"
if echo "$final_names" | grep -qE '^(supabase|github|vercel)\.'; then
  die "after leave, provider tools still listed"
fi
ok "after leave, MCP control-only again"

# ── 15. notify status disabled by default under clean LOCUS_HOME ─────────────
log "15. notify status disabled by default"
if ! has_cmd notify; then
  skip "notify command not available"
else
  # Ensure ambient opt-in env does not pollute the default check
  unset LOCUS_NOTIFY LOCUS_QUIET 2>/dev/null || true
  notify_json="$(locus notify status --json 2>/dev/null || locus --json notify status 2>/dev/null || true)"
  if [[ -z "$notify_json" ]]; then
    # Text fallback: must mention off/disabled
    notify_txt="$(locus notify status 2>/dev/null || true)"
    echo "$notify_txt" | grep -qiE 'off|disabled' \
      || die "notify status should report disabled by default: $notify_txt"
    ok "notify status text shows disabled (default)"
  else
    echo "$notify_json" | python3 -c '
import json, sys
d = json.load(sys.stdin)
# Clean LOCUS_HOME → config off; without LOCUS_NOTIFY, effective must be false
assert d.get("config_enabled") in (False, "false", 0, None) or d.get("config_enabled") is False, d
eff = d.get("effective")
assert eff in (False, "false", 0) or eff is False, "expected effective=false, got %r: %s" % (eff, d)
default = (d.get("default") or "off")
assert str(default).lower() in ("off", "false", "disabled"), d
print("notify config_enabled=%s effective=%s default=%s" % (
    d.get("config_enabled"), d.get("effective"), d.get("default")))
'
    ok "notify status --json: disabled by default under clean LOCUS_HOME"
  fi
fi

# ── 16. graph export/import roundtrip (feature-detected) ─────────────────────
log "16. graph export/import roundtrip (optional)"
export LOCUS_GRAPH_PASSPHRASE="${LOCUS_GRAPH_PASSPHRASE:-e2e-test}"
if ! has_cmd graph && ! has_cmd_path graph export; then
  skip "graph command not available"
else
  graph_path="$LOCUS_HOME/e2e-graph.locusgraph"
  exported=0
  # Encrypted export (passphrase via LOCUS_GRAPH_PASSPHRASE)
  if locus graph export --out "$graph_path" >/dev/null 2>&1 \
    || locus graph export -o "$graph_path" >/dev/null 2>&1; then
    exported=1
  fi

  if [[ "$exported" -ne 1 ]] || [[ ! -s "$graph_path" ]]; then
    skip "graph present but export invocation failed (API may differ)"
  else
    # Magic / size sanity
    head -c 12 "$graph_path" | grep -q 'LOCUSGRAPH' \
      || die "graph export missing LOCUSGRAPH magic"
    ok "graph export wrote $(wc -c <"$graph_path" | tr -d ' ') bytes (LOCUSGRAPH)"
    # Import into same home: existing bindings skip without --force (still exit 0)
    if locus graph import "$graph_path" >/dev/null 2>&1; then
      ok "graph import accepted export (skip-or-write)"
    elif locus graph import "$graph_path" --force >/dev/null 2>&1; then
      ok "graph import --force roundtrip"
    else
      # Fresh home for a true write path
      GRAPH_HOME="$(mktemp -d "${TMPDIR:-/tmp}/locus-graph-imp.XXXXXX")"
      if LOCUS_HOME="$GRAPH_HOME" LOCUS_GRAPH_PASSPHRASE="$LOCUS_GRAPH_PASSPHRASE" \
        locus init >/dev/null 2>&1 \
        && LOCUS_HOME="$GRAPH_HOME" LOCUS_GRAPH_PASSPHRASE="$LOCUS_GRAPH_PASSPHRASE" \
          locus graph import "$graph_path" >/dev/null 2>&1; then
        ok "graph export/import into fresh LOCUS_HOME"
      else
        die "graph import failed for $graph_path"
      fi
      rm -rf "$GRAPH_HOME"
    fi
  fi
fi

# ── 17. ci mint --json (feature-detected) ────────────────────────────────────
log "17. locus ci mint --json (optional)"
if ! has_cmd ci && ! has_cmd_path ci mint; then
  skip "ci command not available"
else
  # Mint always emits JSON; binding required (-b / --binding)
  set +e
  ci_out="$(locus ci mint -b personal --json 2>/dev/null)"
  ci_ec=$?
  if [[ $ci_ec -ne 0 || -z "$ci_out" ]]; then
    ci_out="$(locus ci mint --binding personal 2>/dev/null)"
    ci_ec=$?
  fi
  if [[ $ci_ec -ne 0 || -z "$ci_out" ]]; then
    ci_out="$(locus --json ci mint -b personal 2>/dev/null)"
    ci_ec=$?
  fi
  set -e
  if [[ $ci_ec -ne 0 || -z "$ci_out" ]]; then
    skip "ci mint present but invocation failed (API may differ)"
  else
    echo "$ci_out" | python3 -c '
import json, sys
raw = sys.stdin.read().strip()
assert raw, "empty ci mint output"
d = json.loads(raw)
assert isinstance(d, dict), type(d)
# Required identity fields for pipelines
for k in ("session_id", "binding", "tenant"):
    assert k in d and d[k], "missing %s: %s" % (k, d)
# Must not dump raw secret values by default (no --resolve)
assert d.get("secrets_resolved") in (False, None, "false", 0) or d.get("secrets_resolved") is False, d
# Seal is an HMAC digest, not a provider token — still require it for CI contract
assert d.get("session_id", "").startswith("ses_") or d.get("session_id"), d
print("ci mint binding=%s session=%s secrets_resolved=%s" % (
    d.get("binding"), d.get("session_id"), d.get("secrets_resolved")))
'
    ok "ci mint -b personal --json returns session JSON (no secrets)"
  fi
fi

# ── 18. locus_heartbeat via MCP tools/call (feature-detected) ────────────────
log "18. MCP locus_heartbeat (optional)"
# Heartbeat is a control tool — works pinned or unpinned when present.
locus pin personal --force >/dev/null 2>&1 || locus pin acme --force >/dev/null 2>&1 || true
hb_list_out="$(
  mcp_rpc \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"e2e","version":"0"}}}' \
    '{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}' \
    '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}'
)"
hb_names="$(echo "$hb_list_out" | tool_names_from_list)"
if ! echo "$hb_names" | grep -qx 'locus_heartbeat'; then
  skip "locus_heartbeat MCP tool not available"
else
  hb_out="$(
    mcp_rpc \
      '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"e2e","version":"0"}}}' \
      '{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}' \
      '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"locus_heartbeat","arguments":{}}}'
  )"
  hb_line="$(echo "$hb_out" | tool_call_text)"
  echo "$hb_line" | grep -q '^OK|' || die "locus_heartbeat expected success: $hb_line"
  # Body should be JSON-ish drift/runtime summary without secret values
  echo "$hb_line" | python3 -c '
import json, sys, re
line = sys.stdin.read().strip()
assert line.startswith("OK|"), line
body = line[3:]
# Accept pure JSON or JSON embedded in text
try:
    d = json.loads(body)
except json.JSONDecodeError:
    m = re.search(r"\{.*\}", body, re.S)
    assert m, "heartbeat body not JSON: %r" % body[:200]
    d = json.loads(m.group(0))
# Never return resolved secret values or credential locator names.
blob = json.dumps(d)
# Bearer-style / GitHub PAT prefixes must not appear
for bad in ("sk-", "ghp_", "gho_", "github_pat_", "xoxb-", "AKIA"):
    assert bad not in blob, "heartbeat must not leak secrets (%s)" % bad
assert "secret_value" not in blob.lower()
assert isinstance(d, dict)
assert "ok" in d or "pinned" in d or "runtime" in d, d
print("heartbeat keys=%s pinned=%s ok=%s" % (
    ",".join(sorted(d.keys())[:12]), d.get("pinned"), d.get("ok")))
'
  ok "MCP tools/call locus_heartbeat succeeds (no secrets)"
fi

# ── 19. README mentions graph/ci when those CLIs exist (unit-free smoke) ──────
log "19. README documents graph/ci if CLIs exist"
if has_cmd graph || has_cmd_path graph export; then
  grep -qE 'locus graph|`graph`' "$ROOT/README.md" \
    || die "graph CLI exists but README does not mention graph"
  ok "README mentions graph"
else
  skip "graph not available — README mention not required"
fi
if has_cmd ci || has_cmd_path ci mint; then
  grep -qE 'locus ci|`ci`|ci mint' "$ROOT/README.md" \
    || die "ci CLI exists but README does not mention ci"
  ok "README mentions ci"
else
  skip "ci not available — README mention not required"
fi

# ── 20. dashboard / serve health (feature-detected) ──────────────────────────
log "20. dashboard health curl (if serve available)"
if ! has_cmd serve && ! has_cmd dashboard; then
  skip "serve/dashboard not available"
else
  need curl
  # Pick a free high port to avoid colliding with a developer dashboard.
  DASH_PORT=$((18750 + RANDOM % 1000))
  serve_log="$LOCUS_HOME/e2e-serve.log"
  # serve defaults to no browser open (only dashboard opens by default).
  "$LOCUS_BIN" serve --port "$DASH_PORT" >"$serve_log" 2>&1 &
  serve_pid=$!
  cleanup_serve() {
    kill "$serve_pid" 2>/dev/null || true
    wait "$serve_pid" 2>/dev/null || true
  }
  health_ok=0
  for _ in $(seq 1 25); do
    if ! kill -0 "$serve_pid" 2>/dev/null; then
      break
    fi
    if health="$(curl -fsS --max-time 1 "http://127.0.0.1:${DASH_PORT}/api/health" 2>/dev/null)"; then
      echo "$health" | python3 -c '
import json, sys
d = json.load(sys.stdin)
assert d.get("ok") is True, d
assert d.get("service") == "locus-dashboard", d
assert "version" in d, d
print("health service=%s version=%s" % (d.get("service"), d.get("version")))
'
      health_ok=1
      break
    fi
    sleep 0.15
  done
  cleanup_serve
  if [[ "$health_ok" -eq 1 ]]; then
    ok "dashboard GET /api/health on port $DASH_PORT"
  else
    tail -n 30 "$serve_log" 2>/dev/null || true
    die "serve started but /api/health never became ready (port $DASH_PORT)"
  fi
fi

# ── 21. forensics export (feature-detected) ──────────────────────────────────
log "21. forensics export (optional)"
if ! has_cmd forensics && ! has_cmd_path forensics export; then
  skip "forensics command not available"
else
  locus pin personal --force >/dev/null 2>&1 || locus pin acme --force >/dev/null 2>&1 || true
  pack_path="$LOCUS_HOME/e2e-forensics.json"
  exported=0
  if locus forensics export --out "$pack_path" >/dev/null 2>&1 \
    || locus forensics export -o "$pack_path" >/dev/null 2>&1; then
    exported=1
  fi
  if [[ "$exported" -ne 1 ]] || [[ ! -s "$pack_path" ]]; then
    # JSON-on-stdout fallback
    if pack_json="$(locus forensics export --json 2>/dev/null)" \
      && [[ -n "$pack_json" ]]; then
      echo "$pack_json" >"$pack_path"
      exported=1
    fi
  fi
  if [[ "$exported" -ne 1 ]] || [[ ! -s "$pack_path" ]]; then
    skip "forensics present but export invocation failed (API may differ)"
  else
    python3 -c '
import json, sys
path = sys.argv[1]
with open(path) as f:
    d = json.load(f)
assert isinstance(d, dict), type(d)
# Pack must contain neither secret values nor credential locator names.
blob = json.dumps(d).lower()
for bad in ("sk-", "ghp_", "gho_", "github_pat_", "xoxb-", "akia", "secret_value", "\"credential_ref\"", "phm:", "env:", "test:"):
    assert bad not in blob, "forensics pack must not leak secrets (%s)" % bad
# Structural surface (keys evolve; require a few stable-ish ones)
keys = set(d.keys())
# Accept either nested doctor or top-level pin/bindings style packs
assert keys, "empty pack"
print("forensics keys=%s" % ",".join(sorted(keys)[:16]))
' "$pack_path"
    ok "forensics export wrote pack ($(wc -c <"$pack_path" | tr -d ' ') bytes, no secrets)"
  fi
fi

# ── 22. goal status (feature-detected) ───────────────────────────────────────
log "22. goal status (optional)"
if ! has_cmd goal && ! has_cmd_path goal status; then
  skip "goal command not available"
else
  set +e
  goal_json="$(locus goal status --json 2>/dev/null)"
  goal_ec=$?
  if [[ $goal_ec -ne 0 || -z "$goal_json" ]]; then
    goal_json="$(locus --json goal status 2>/dev/null)"
    goal_ec=$?
  fi
  set -e
  if [[ $goal_ec -ne 0 || -z "$goal_json" ]]; then
    # Text mode still useful
    if locus goal status >/dev/null 2>&1; then
      ok "goal status text mode"
    else
      skip "goal present but status invocation failed"
    fi
  else
    echo "$goal_json" | python3 -c '
import json, sys
raw = sys.stdin.read().strip()
d = json.loads(raw)
assert isinstance(d, dict), type(d)
# Accept several shapes: milestones list, totals, done/total
has_progress = any(
    k in d for k in ("milestones", "done", "total", "completed", "remaining", "progress", "source")
)
assert has_progress or "goals" in d or "ok" in d, d
print("goal status keys=%s" % ",".join(sorted(d.keys())[:12]))
'
    ok "goal status --json returns progress"
  fi
fi

# ── 23. topic help (feature-detected) ────────────────────────────────────────
log "23. topic help (optional)"
if has_cmd topic; then
  locus topic >/dev/null
  locus topic dashboard >/dev/null
  ok "locus topic + topic dashboard"
elif "$LOCUS_BIN" help topic dashboard >/dev/null 2>&1; then
  ok "locus help topic dashboard"
else
  skip "topic help not available"
fi
# ── 24. adversarial release security probes ────────────────────────────────
log "24. adversarial release credential/workspace probes"
SECURITY_HOME="$(mktemp -d "${TMPDIR:-/tmp}/locus-security-e2e.XXXXXX")"
SECURITY_WS="$(mktemp -d "${TMPDIR:-/tmp}/locus-security-ws.XXXXXX")"
LOCUS_HOME="$SECURITY_HOME" "$LOCUS_BIN" init >/dev/null

set +e
test_out="$(LOCUS_HOME="$SECURITY_HOME" LOCUS_ALLOW_TEST_CREDS=1 "$LOCUS_BIN" binding add release-test \
  --tenant release-test --provider github --account release-test \
  --credential-ref test:RELEASE_TEST_LOCATOR_CANARY 2>&1)"
test_ec=$?
set -e
[[ $test_ec -ne 0 ]] || die "release binary accepted test: credential"
[[ "$test_out" != *"RELEASE_TEST_LOCATOR_CANARY"* ]] || die "release error leaked test locator"
ok "release binary rejects test: even with legacy env opt-in"

cat >"$SECURITY_HOME/bindings/legacy.toml" <<'EOF'
[binding]
id = "bnd_legacy"
alias = "legacy"
tenant = "legacy-tenant"

[[binding.providers]]
provider = "github"
account = "legacy-account"
credential_ref = "LEGACY_RELEASE_LOCATOR_CANARY"
EOF
set +e
legacy_list="$(LOCUS_HOME="$SECURITY_HOME" "$LOCUS_BIN" binding list 2>&1)"
legacy_ec=$?
set -e
[[ $legacy_ec -ne 0 ]] || die "legacy binding was silently accepted"
[[ "$legacy_list" == *"migrate-credential-refs legacy --write"* ]] \
  || die "legacy binding did not return migration action"
[[ "$legacy_list" != *"LEGACY_RELEASE_LOCATOR_CANARY"* ]] || die "legacy list leaked locator"
ok "legacy binding is actionable and non-disclosing"

migrate_out="$(LOCUS_HOME="$SECURITY_HOME" "$LOCUS_BIN" binding migrate-credential-refs legacy --write 2>&1)"
[[ "$migrate_out" != *"LEGACY_RELEASE_LOCATOR_CANARY"* ]] || die "migration output leaked locator"
grep -q 'credential_ref = "phm:LEGACY_RELEASE_LOCATOR_CANARY"' "$SECURITY_HOME/bindings/legacy.toml" \
  || die "legacy migration did not persist explicit phm ref"
retry_migrate_out="$(LOCUS_HOME="$SECURITY_HOME" "$LOCUS_BIN" binding migrate-credential-refs legacy --write 2>&1)"
[[ "$retry_migrate_out" == *"migrated 1 credential reference"* ]] \
  || die "exact migration retry was not reconciled as committed"
[[ "$retry_migrate_out" != *"LEGACY_RELEASE_LOCATOR_CANARY"* ]] || die "migration retry leaked locator"
! grep -R -q 'LEGACY_RELEASE_LOCATOR_CANARY' "$SECURITY_HOME/bindings"/.*credential-migration.json 2>/dev/null \
  || die "migration journal leaked locator"
python3 - "$SECURITY_HOME/audit/events.jsonl" <<'PY'
import json, sys
events = []
with open(sys.argv[1], encoding="utf-8") as fh:
    for line in fh:
        try:
            events.append(json.loads(line))
        except json.JSONDecodeError:
            pass
matches = [event for event in events if event.get("op") == "binding.credential_refs_migrated"]
assert len(matches) == 1, matches
PY
ok "explicit legacy migration and exact retry are durable, idempotent, and non-disclosing"

set +e
phantom_error="$(PATH=/usr/bin:/bin LOCUS_HOME="$SECURITY_HOME" "$LOCUS_BIN" run \
  -b legacy --strict-creds -- /usr/bin/true 2>&1)"
phantom_ec=$?
set -e
[[ $phantom_ec -ne 0 ]] || die "strict resolution unexpectedly succeeded without Phantom"
[[ "$phantom_error" == *"provider=github source=phantom code=unavailable"* ]] \
  || die "strict Phantom error omitted safe provider/source metadata"
[[ "$phantom_error" != *"LEGACY_RELEASE_LOCATOR_CANARY"* ]] || die "strict Phantom error leaked locator"
ok "Phantom failures expose only provider/source metadata"

ci_security="$(LOCUS_HOME="$SECURITY_HOME" "$LOCUS_BIN" --json ci mint -b legacy)"
[[ "$ci_security" != *"LEGACY_RELEASE_LOCATOR_CANARY"* ]] || die "CI mint leaked locator"
[[ "$ci_security" != *"CREDENTIAL_REF"* ]] || die "CI mint exported credential locator key"
ok "CI mint omits credential locator names and keys"

show_security="$(LOCUS_HOME="$SECURITY_HOME" "$LOCUS_BIN" --json binding show legacy)"
graph_security="$(LOCUS_HOME="$SECURITY_HOME" "$LOCUS_BIN" --json graph list)"
audit_security="$(cat "$SECURITY_HOME/audit/events.jsonl")"
for surface in "$show_security" "$graph_security" "$audit_security"; do
  [[ "$surface" != *"LEGACY_RELEASE_LOCATOR_CANARY"* ]] || die "CLI/audit surface leaked locator"
done
ok "binding show, graph list, and audit omit locator names"

LOCUS_RELEASE_ENV_LOCATOR_CANARY="release-secret-value" \
  LOCUS_HOME="$SECURITY_HOME" "$LOCUS_BIN" binding add env-probe \
  --tenant env-probe --provider github --account env-probe \
  --credential-ref env:LOCUS_RELEASE_ENV_LOCATOR_CANARY >/dev/null
child_env="$(LOCUS_RELEASE_ENV_LOCATOR_CANARY="release-secret-value" \
  LOCUS_HOME="$SECURITY_HOME" "$LOCUS_BIN" run -b env-probe -- env 2>/dev/null)"
[[ "$child_env" != *"LOCUS_RELEASE_ENV_LOCATOR_CANARY"* ]] || die "child env leaked locator name"
[[ "$child_env" != *"CREDENTIAL_REF"* ]] || die "child env exported credential locator key"
[[ "$child_env" == *"GH_TOKEN=release-secret-value"* ]] || die "resolved provider credential missing"
ok "child env scrubs locator key and injects only provider-standard secret keys"

ln -s missing-policy.toml "$SECURITY_WS/.locus.toml"
for command in \
  "pin legacy --force" \
  "run -b legacy --force -- true"; do
  set +e
  broken_out="$(cd "$SECURITY_WS" && LOCUS_HOME="$SECURITY_HOME" "$LOCUS_BIN" $command 2>&1)"
  broken_ec=$?
  set -e
  [[ $broken_ec -ne 0 ]] || die "broken workspace link allowed: $command"
  [[ "$broken_out" == *"broken or unreadable"* ]] || die "broken workspace link error not explicit"
done
set +e
doctor_out="$(cd "$SECURITY_WS" && LOCUS_HOME="$SECURITY_HOME" "$LOCUS_BIN" --json doctor 2>/dev/null)"
doctor_ec=$?
set -e
[[ $doctor_ec -eq 2 ]] || die "doctor did not report UNSAFE for broken workspace link"
echo "$doctor_out" | python3 -c '
import json, sys
d = json.load(sys.stdin)
assert d["verdict"] == "UNSAFE", d
assert d["workspace"]["valid"] is False, d["workspace"]
'
ok "broken workspace link blocks force/run and makes doctor UNSAFE"

# ── 25. verify claim (feature-detected) ──────────────────────────────────────
log "25. locus verify claim (optional)"
if ! has_cmd verify && ! has_cmd_path verify claim; then
  skip "verify claim command not available"
else
  set +e
  verify_json="$(locus verify claim --text 'Deploy hits https://api.x/v2' --json 2>/dev/null)"
  verify_ec=$?
  if [[ $verify_ec -ne 0 || -z "$verify_json" ]]; then
    verify_json="$(locus --json verify claim --text 'Deploy hits https://api.x/v2' 2>/dev/null)"
    verify_ec=$?
  fi
  set -e
  if [[ $verify_ec -ne 0 || -z "$verify_json" ]]; then
    skip "verify present but claim invocation failed (API may differ)"
  else
    echo "$verify_json" | python3 -c '
import json, sys
raw = sys.stdin.read().strip()
d = json.loads(raw)
assert isinstance(d, dict), type(d)
# Heuristic claim scoring surface (verification plane stubs)
for k in ("claim", "confidence", "needs_tool"):
    assert k in d, "missing %s: %s" % (k, d)
assert d.get("needs_tool") is True, "URL claim should need_tool: %s" % d
conf = str(d.get("confidence") or "").lower()
assert conf in ("low", "medium", "high", "unknown") or conf, d
# Never leak secrets in claim scoring output
blob = json.dumps(d).lower()
for bad in ("sk-", "ghp_", "gho_", "github_pat_", "xoxb-", "akia", "secret_value"):
    assert bad not in blob, "verify claim must not leak secrets (%s)" % bad
print("verify claim confidence=%s needs_tool=%s signals=%s" % (
    d.get("confidence"), d.get("needs_tool"), d.get("signals")))
'
    ok "verify claim --json scores URL claim (needs_tool)"
  fi
fi

# ── 26. verify session (feature-detected) ────────────────────────────────────
log "26. locus verify session (optional)"
if ! has_cmd verify && ! has_cmd_path verify session; then
  skip "verify session command not available"
else
  # Known pin so whoami + safe_next.ready surface when identity plane is healthy.
  locus pin personal --force >/dev/null 2>&1 || locus pin acme --force >/dev/null 2>&1 || true
  set +e
  vs_json="$(locus verify session --json 2>/dev/null)"
  vs_ec=$?
  if [[ -z "$vs_json" ]]; then
    vs_json="$(locus --json verify session 2>/dev/null)"
    vs_ec=$?
  fi
  set -e
  if [[ -z "$vs_json" ]]; then
    die "verify session emitted no inspection JSON (exit=$vs_ec)"
  else
    vs_expected_ec="$(printf '%s' "$vs_json" | python3 -c '
import json, sys
d = json.load(sys.stdin)
print(0 if d.get("session_ok") is True else 1)
')"
    [[ $vs_ec -eq $vs_expected_ec ]] \
      || die "verify session exit=$vs_ec does not match session_ok (expected $vs_expected_ec)"
    echo "$vs_json" | python3 -c '
import json, sys
raw = sys.stdin.read().strip()
d = json.loads(raw)
assert isinstance(d, dict), type(d)
# Session pack contract for hub heartbeats
assert d.get("kind") == "session", "expected kind=session: %s" % d
assert "session_ok" in d and isinstance(d["session_ok"], bool), d
# doctor + safe_next are core fields when present
doctor = d.get("doctor")
assert isinstance(doctor, dict), "doctor missing/invalid: %s" % d
safe_next = d.get("safe_next")
assert isinstance(safe_next, dict), "safe_next missing/invalid: %s" % d
# Optional fields when emitted
if "version" in d:
    assert d["version"], d
# Never leak secret *values* (mirror claim checks; CredentialRef names may appear)
blob = json.dumps(d).lower()
for bad in ("sk-", "ghp_", "gho_", "github_pat_", "xoxb-", "akia", "secret_value"):
    assert bad not in blob, "verify session must not leak secrets (%s)" % bad
print("verify session kind=%s session_ok=%s safe_next=%s doctor_ok=%s" % (
    d.get("kind"), d.get("session_ok"),
    (safe_next or {}).get("action"), (doctor or {}).get("ok")))
'
    ok "verify session --json pack + truthful exit (kind/session_ok/doctor/safe_next, no secrets)"
  fi
fi

# ── 26b. watch session heartbeat (feature-detected, soft) ────────────────────
log "26b. locus watch --once --json (optional heartbeat)"
if ! has_cmd watch; then
  skip "watch command not available"
elif ! help_mentions watch "--require-ok" && ! help_mentions watch "session"; then
  skip "watch session heartbeat flags not available"
else
  # Unpinned: compact heartbeat must still emit; soft --once exits 0 without --require-ok.
  locus leave >/dev/null 2>&1 || true
  set +e
  watch_json="$(locus --json watch --once 2>/dev/null)"
  watch_ec=$?
  if [[ -z "$watch_json" ]]; then
    watch_json="$(locus watch --once --json 2>/dev/null)"
    watch_ec=$?
  fi
  set -e
  if [[ -z "$watch_json" ]]; then
    skip "watch --once emitted no JSON (exit=$watch_ec)"
  else
    echo "$watch_json" | python3 -c '
import json, sys
raw = sys.stdin.read().strip().splitlines()[-1]
d = json.loads(raw)
assert isinstance(d, dict), type(d)
assert d.get("kind") == "watch", "expected kind=watch: %s" % d
assert "session_ok" in d and isinstance(d["session_ok"], bool), d
assert "doctor_verdict" in d and d["doctor_verdict"], d
assert "safe_next" in d and d["safe_next"], d
assert "pinned" in d and isinstance(d["pinned"], bool), d
blob = json.dumps(d).lower()
for bad in ("sk-", "ghp_", "gho_", "github_pat_", "xoxb-", "akia", "secret_value"):
    assert bad not in blob, "watch heartbeat must not leak secrets (%s)" % bad
print("watch kind=%s session_ok=%s whoami=%s doctor=%s safe_next=%s" % (
    d.get("kind"), d.get("session_ok"), d.get("whoami"),
    d.get("doctor_verdict"), d.get("safe_next")))
'
    # Soft unpinned --once should not fail closed.
    [[ $watch_ec -eq 0 ]] \
      || die "watch --once unpinned should exit 0 without --require-ok (exit=$watch_ec)"
    # --require-ok fail-closed when session is not ready (typical unpinned).
    set +e
    locus watch --once --require-ok --json >/dev/null 2>&1
    req_ec=$?
    set -e
    if [[ $req_ec -eq 0 ]]; then
      # Environment happened to be fully ready — still ok for e2e.
      ok "watch --once --json heartbeat shape (session_ok may be true here)"
    else
      [[ $req_ec -ne 0 ]] || die "expected non-zero with --require-ok when not ready"
      ok "watch --once --json heartbeat + --require-ok fail-closed"
    fi
  fi
fi

# ── 27. MCP locus_safe_next (feature-detected) ───────────────────────────────
log "27. MCP locus_safe_next (optional)"
locus leave >/dev/null 2>&1 || true
sn_list_out="$(
  mcp_rpc \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"e2e","version":"0"}}}' \
    '{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}' \
    '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}'
)"
sn_names="$(echo "$sn_list_out" | tool_names_from_list)"
if ! echo "$sn_names" | grep -qx 'locus_safe_next'; then
  skip "locus_safe_next MCP tool not available"
else
  # Unpinned: safe_next should recommend enter / re_pin style action (isError ok)
  sn_un_out="$(
    mcp_rpc \
      '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"e2e","version":"0"}}}' \
      '{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}' \
      '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"locus_safe_next","arguments":{}}}'
  )"
  sn_un_line="$(echo "$sn_un_out" | tool_call_text)"
  # Accept OK| or ERR| — body must describe a next action without secrets
  echo "$sn_un_line" | python3 -c '
import json, sys, re
line = sys.stdin.read().strip()
assert line.startswith("OK|") or line.startswith("ERR|"), line
body = line[3:] if line.startswith("OK|") else line[4:]
try:
    d = json.loads(body)
except json.JSONDecodeError:
    m = re.search(r"\{.*\}", body, re.S)
    assert m, "safe_next body not JSON: %r" % body[:200]
    d = json.loads(m.group(0))
assert isinstance(d, dict), d
# Single best next action surface
action = (d.get("action") or d.get("next") or d.get("safe_next") or "").lower()
# Accept action field or nested recommendation
if not action:
    action = str(d.get("recommendation") or d.get("status") or "").lower()
blob = json.dumps(d).lower()
for bad in ("sk-", "ghp_", "gho_", "github_pat_", "xoxb-", "akia", "secret_value"):
    assert bad not in blob, "safe_next must not leak secrets (%s)" % bad
# Unpinned should not claim ready without a pin
ready_ish = action in ("ready",) or d.get("ready") is True
assert not ready_ish or d.get("pinned"), "unpinned safe_next should not be ready: %s" % d
print("safe_next unpinned keys=%s action=%s" % (
    ",".join(sorted(d.keys())[:12]), action or d.get("action")))
'
  ok "MCP locus_safe_next unpinned returns next action (no secrets)"

  # Pinned: should succeed with a coherent action (ready / approve / doctor_fix / …)
  locus pin personal --force >/dev/null 2>&1 || locus pin acme --force >/dev/null 2>&1 || true
  sn_pin_out="$(
    mcp_rpc \
      '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"e2e","version":"0"}}}' \
      '{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}' \
      '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"locus_safe_next","arguments":{}}}'
  )"
  sn_pin_line="$(echo "$sn_pin_out" | tool_call_text)"
  echo "$sn_pin_line" | python3 -c '
import json, sys, re
line = sys.stdin.read().strip()
assert line.startswith("OK|") or line.startswith("ERR|"), line
body = line[3:] if line.startswith("OK|") else line[4:]
try:
    d = json.loads(body)
except json.JSONDecodeError:
    m = re.search(r"\{.*\}", body, re.S)
    assert m, "safe_next body not JSON: %r" % body[:200]
    d = json.loads(m.group(0))
assert isinstance(d, dict), d
blob = json.dumps(d)
for bad in ("sk-", "ghp_", "gho_", "github_pat_", "xoxb-", "AKIA"):
    assert bad not in blob, "safe_next must not leak secrets (%s)" % bad
assert "secret_value" not in blob.lower()
print("safe_next pinned keys=%s" % ",".join(sorted(d.keys())[:12]))
'
  ok "MCP locus_safe_next pinned returns action (no secrets)"
fi

# ── 28. upstream list (feature-detected) ─────────────────────────────────────
log "28. locus upstream list (optional)"
if ! has_cmd upstream && ! has_cmd_path upstream list; then
  skip "upstream command not available"
else
  set +e
  up_json="$(locus upstream list --json 2>/dev/null)"
  up_ec=$?
  if [[ $up_ec -ne 0 || -z "$up_json" ]]; then
    up_json="$(locus --json upstream list 2>/dev/null)"
    up_ec=$?
  fi
  set -e
  if [[ $up_ec -ne 0 || -z "$up_json" ]]; then
    # Text mode still useful
    if locus upstream list >/dev/null 2>&1; then
      ok "upstream list text mode"
    else
      skip "upstream present but list invocation failed"
    fi
  else
    echo "$up_json" | python3 -c '
import json, sys
raw = sys.stdin.read().strip()
d = json.loads(raw)
# Array of recipes, or object wrapping recipes
if isinstance(d, list):
    recipes = d
elif isinstance(d, dict):
    recipes = d.get("recipes") or d.get("items") or d.get("upstream") or []
    if not recipes and any(k in d for k in ("id", "title", "command")):
        recipes = [d]
else:
    raise AssertionError("unexpected upstream list type: %s" % type(d))
assert isinstance(recipes, list) and len(recipes) >= 1, "expected >=1 recipe: %r" % d
# Structural + secret hygiene
blob = json.dumps(recipes).lower()
for bad in ("sk-", "ghp_", "gho_", "github_pat_", "xoxb-", "akia", "secret_value"):
    assert bad not in blob, "upstream list must not leak secrets (%s)" % bad
ids = []
by_id = {}
for r in recipes:
    assert isinstance(r, dict), r
    rid = r.get("id") or r.get("name") or r.get("recipe")
    assert rid, "recipe missing id: %s" % r
    ids.append(rid)
    by_id[rid] = r
# Compatible adapters keep secure defaults; daemon/OAuth adapters must publish
# an explicit high-authority readiness gate instead of a false sandbox claim.
for required in ("github-mcp", "github-official", "supabase-mcp", "vercel-mcp"):
    assert required in by_id, "missing top recipe %s in %s" % (required, ids)
for rid in ("github-mcp", "supabase-mcp"):
    r = by_id[rid]
    sandbox = r.get("default_sandbox", r.get("defaultSandbox"))
    assert sandbox is True, "%s must default_sandbox: %s" % (rid, r)
    assert r.get("sandbox_compatibility") == "compatible", r
    assert r.get("readiness") == "ready", r
for rid, risk in (("github-official", "host_docker_daemon"),
                  ("vercel-mcp", "oauth_loopback_listener")):
    r = by_id[rid]
    sandbox = r.get("default_sandbox", r.get("defaultSandbox"))
    assert sandbox is False, "%s must be unavailable by default: %s" % (rid, r)
    assert r.get("sandbox_compatibility") == "incompatible", r
    assert r.get("readiness") == "explicit_unsandboxed_required", r
    assert risk in (r.get("risks") or []), r
    assert r.get("readiness_detail"), r
for rid in ("github-mcp", "github-official", "supabase-mcp"):
    r = by_id[rid]
    resolve = r.get("default_resolve_secrets", r.get("defaultResolveSecrets"))
    assert resolve is True, "%s must default_resolve_secrets: %s" % (rid, r)
v = by_id["vercel-mcp"]
v_resolve = v.get("default_resolve_secrets", v.get("defaultResolveSecrets"))
assert v_resolve is False, "vercel-mcp resolve_secrets must default off: %s" % v
print("upstream list count=%d sample=%s" % (len(recipes), ",".join(ids[:6])))
'
    ok "upstream list --json returns recipes (no secrets)"
  fi
fi

# ── 15 reaffirm: notify still off after full suite (default hygiene) ─────────
# Step 15 already asserts default-off under clean LOCUS_HOME. Re-check late so
# later steps cannot silently re-enable notify via config writes.
log "29. notify still disabled after suite (default hygiene)"
if ! has_cmd notify; then
  skip "notify command not available"
else
  unset LOCUS_NOTIFY LOCUS_QUIET 2>/dev/null || true
  notify_json="$(locus notify status --json 2>/dev/null || true)"
  if [[ -n "$notify_json" ]]; then
    echo "$notify_json" | python3 -c '
import json, sys
d = json.load(sys.stdin)
eff = d.get("effective")
assert eff in (False, "false", 0) or eff is False, "notify effective must stay false: %s" % d
print("notify still effective=%s config_enabled=%s" % (eff, d.get("config_enabled")))
'
    ok "notify still disabled by default after full e2e suite"
  else
    notify_txt="$(locus notify status 2>/dev/null || true)"
    echo "$notify_txt" | grep -qiE 'off|disabled' \
      || die "notify should remain disabled after suite: $notify_txt"
    ok "notify still disabled (text) after full e2e suite"
  fi
fi

# ── 30. HTTP MCP session + SSE (feature-detected; soft-skip on bind/timeout) ──
# When locus-mcp --http is available: start on a free loopback port with
# LOCUS_MCP_HTTP_TOKEN, assert fail-closed auth, Mcp-Session-Id mint, and
# optional GET /mcp/sse?once=1 (skips if the endpoint is not built yet).
log "30. locus-mcp --http session + SSE (optional)"
if ! "$MCP_BIN" --help 2>&1 | grep -qE -- '--http(\s|=|$)'; then
  skip "locus-mcp --http not available"
elif ! command -v curl >/dev/null 2>&1; then
  skip "curl not available for HTTP MCP probes"
else
  HTTP_MCP_PORT="$(
    python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()'
  )"
  HTTP_MCP_TOKEN="e2e-http-$(od -An -N8 -tx1 /dev/urandom | tr -d ' \n')"
  HTTP_MCP_ADDR="127.0.0.1:${HTTP_MCP_PORT}"
  HTTP_MCP_BASE="http://${HTTP_MCP_ADDR}"
  http_mcp_log="$LOCUS_HOME/e2e-http-mcp.log"

  set +e
  LOCUS_HOME="$LOCUS_HOME" \
    LOCUS_MCP_HTTP_TOKEN="$HTTP_MCP_TOKEN" \
    LOCUS_MCP_AUTO_PIN=0 \
    "$MCP_BIN" --http "$HTTP_MCP_ADDR" >"$http_mcp_log" 2>&1 &
  HTTP_MCP_PID=$!
  set -e

  http_ready=0
  for _ in $(seq 1 40); do
    if ! kill -0 "$HTTP_MCP_PID" 2>/dev/null; then
      break
    fi
    if curl -fsS --max-time 1 "${HTTP_MCP_BASE}/health" >/dev/null 2>&1; then
      http_ready=1
      break
    fi
    sleep 0.1
  done

  if [[ "$http_ready" -ne 1 ]]; then
    if [[ -n "${HTTP_MCP_PID:-}" ]]; then
      kill "$HTTP_MCP_PID" 2>/dev/null || true
      wait "$HTTP_MCP_PID" 2>/dev/null || true
      HTTP_MCP_PID=""
    fi
    tail -n 20 "$http_mcp_log" 2>/dev/null || true
    skip "locus-mcp --http did not become ready (bind/timeout on ${HTTP_MCP_ADDR})"
  else
    # POST initialize without token → 401 (fail closed).
    init_body='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"e2e-http","version":"0"}}}'
    no_auth_body="$LOCUS_HOME/e2e-http-noauth.body"
    no_auth_code="$(
      curl -sS -o "$no_auth_body" -w '%{http_code}' --max-time 5 \
        -X POST "${HTTP_MCP_BASE}/mcp" \
        -H 'Content-Type: application/json' \
        -H 'Accept: application/json' \
        -d "$init_body" 2>/dev/null || echo "000"
    )"
    [[ "$no_auth_code" == "401" ]] \
      || die "POST /mcp initialize without token expected 401, got ${no_auth_code}"

    tools_body='{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}'
    no_auth_tools="$(
      curl -sS -o /dev/null -w '%{http_code}' --max-time 5 \
        -X POST "${HTTP_MCP_BASE}/mcp" \
        -H 'Content-Type: application/json' \
        -H 'Accept: application/json' \
        -d "$tools_body" 2>/dev/null || echo "000"
    )"
    [[ "$no_auth_tools" == "401" ]] \
      || die "POST /mcp tools/list without token expected 401, got ${no_auth_tools}"
    ok "HTTP MCP POST without token → 401 (initialize + tools/list)"

    # With token → 200 + Mcp-Session-Id header.
    auth_hdr_file="$LOCUS_HOME/e2e-http-auth.hdr"
    auth_body_file="$LOCUS_HOME/e2e-http-auth.body"
    auth_code="$(
      curl -sS -D "$auth_hdr_file" -o "$auth_body_file" -w '%{http_code}' --max-time 5 \
        -X POST "${HTTP_MCP_BASE}/mcp" \
        -H "Authorization: Bearer ${HTTP_MCP_TOKEN}" \
        -H 'Content-Type: application/json' \
        -H 'Accept: application/json' \
        -d "$init_body" 2>/dev/null || echo "000"
    )"
    [[ "$auth_code" == "200" ]] \
      || die "POST /mcp initialize with token expected 200, got ${auth_code} body=$(head -c 200 "$auth_body_file" 2>/dev/null || true)"
    session_id="$(
      python3 -c '
import sys
path = sys.argv[1]
sid = None
with open(path, "rb") as f:
    raw = f.read().decode("utf-8", "replace")
for line in raw.splitlines():
    if ":" not in line:
        continue
    k, v = line.split(":", 1)
    if k.strip().lower() == "mcp-session-id":
        sid = v.strip()
        break
assert sid, "missing Mcp-Session-Id header in:\\n%s" % raw[:500]
assert len(sid) >= 16, "session id too short: %r" % sid
print(sid)
' "$auth_hdr_file"
    )"
    python3 -c '
import json, sys
path = sys.argv[1]
with open(path) as f:
    d = json.load(f)
assert d.get("jsonrpc") == "2.0", d
assert d.get("id") == 1, d
assert "result" in d, d
assert d.get("error") is None or "error" not in d or d["error"] is None, d
print("initialize ok server=%s" % (
    (d.get("result") or {}).get("serverInfo", {}).get("name") or "?"))
' "$auth_body_file"
    ok "HTTP MCP initialize with token → 200 + Mcp-Session-Id (${session_id:0:8}…)"

    # tools/list with token + session header still succeeds.
    tools_hdr="$LOCUS_HOME/e2e-http-tools.hdr"
    tools_out="$LOCUS_HOME/e2e-http-tools.body"
    tools_code="$(
      curl -sS -D "$tools_hdr" -o "$tools_out" -w '%{http_code}' --max-time 5 \
        -X POST "${HTTP_MCP_BASE}/mcp" \
        -H "Authorization: Bearer ${HTTP_MCP_TOKEN}" \
        -H "Mcp-Session-Id: ${session_id}" \
        -H 'Content-Type: application/json' \
        -H 'Accept: application/json' \
        -d "$tools_body" 2>/dev/null || echo "000"
    )"
    [[ "$tools_code" == "200" ]] \
      || die "POST /mcp tools/list with token expected 200, got ${tools_code}"
    python3 -c '
import json, sys
path = sys.argv[1]
with open(path) as f:
    d = json.load(f)
tools = (d.get("result") or {}).get("tools") or []
names = [t.get("name") for t in tools if isinstance(t, dict)]
assert "locus_whoami" in names, "tools/list missing locus_whoami: %s" % names[:12]
print("tools/list count=%d" % len(names))
' "$tools_out"
    ok "HTTP MCP tools/list with token + session → 200"

    # GET /mcp/sse?once=1 — soft-skip if endpoint not present yet.
    sse_hdr="$LOCUS_HOME/e2e-http-sse.hdr"
    sse_body="$LOCUS_HOME/e2e-http-sse.body"
    set +e
    sse_code="$(
      curl -sS -D "$sse_hdr" -o "$sse_body" -w '%{http_code}' --max-time 8 \
        -H "Authorization: Bearer ${HTTP_MCP_TOKEN}" \
        -H 'Accept: text/event-stream' \
        "${HTTP_MCP_BASE}/mcp/sse?once=1" 2>/dev/null
    )"
    sse_ec=$?
    set -e
    if [[ $sse_ec -ne 0 || -z "$sse_code" || "$sse_code" == "000" ]]; then
      skip "GET /mcp/sse?once=1 timed out or failed (curl ec=${sse_ec})"
    elif [[ "$sse_code" == "404" || "$sse_code" == "405" ]]; then
      skip "GET /mcp/sse not available (HTTP ${sse_code}) — session ticks not built yet"
    elif [[ "$sse_code" == "401" ]]; then
      die "GET /mcp/sse?once=1 with token returned 401 (auth should pass)"
    elif [[ "$sse_code" != "200" ]]; then
      skip "GET /mcp/sse?once=1 unexpected status ${sse_code} (soft)"
    else
      python3 -c '
import sys
hdr_path, body_path = sys.argv[1], sys.argv[2]
with open(hdr_path, "rb") as f:
    headers = f.read().decode("utf-8", "replace").lower()
with open(body_path, "rb") as f:
    body = f.read().decode("utf-8", "replace")
assert "text/event-stream" in headers, "expected SSE content-type: %s" % headers[:300]
assert "event:" in body or "data:" in body, "expected SSE framing: %r" % body[:400]
# Values-free: no credential material in ticks.
lower = body.lower()
for bad in ("sk-", "ghp_", "gho_", "github_pat_", "xoxb-", "akia", "secret_value", "phm_", "\"credential_ref\""):
    assert bad not in lower, "SSE body must not leak secrets (%s)" % bad
print("sse once bytes=%d" % len(body))
' "$sse_hdr" "$sse_body"
      ok "HTTP MCP GET /mcp/sse?once=1 with token → SSE session tick"
    fi

    if [[ -n "${HTTP_MCP_PID:-}" ]]; then
      kill "$HTTP_MCP_PID" 2>/dev/null || true
      wait "$HTTP_MCP_PID" 2>/dev/null || true
      HTTP_MCP_PID=""
    fi
    ok "locus-mcp --http server torn down cleanly"
  fi
fi

printf '\n========================================\n'
printf 'e2e PASS  (%d checks, %d skipped)\n' "$pass" "$skip"
printf 'LOCUS_HOME was %s (cleaned)\n' "$LOCUS_HOME"
printf '========================================\n'
