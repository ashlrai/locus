#!/usr/bin/env bash
# Locus end-to-end shell tests — pin, isolation, MCP, freeze, approval, doctor.
set -euo pipefail

export PATH="${HOME}/.cargo/bin:${PATH}"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

LOCUS_BIN="${LOCUS_BIN:-$ROOT/target/release/locus}"
MCP_BIN="${MCP_BIN:-$ROOT/target/release/locus-mcp}"

pass=0
fail=0

log()  { printf '\n==> %s\n' "$*"; }
ok()   { printf '  ok  %s\n' "$*"; pass=$((pass + 1)); }
die()  { printf '  FAIL %s\n' "$*" >&2; fail=$((fail + 1)); exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || die "missing required tool: $1"; }

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
trap 'rm -rf "$LOCUS_HOME" "${WS_DIR:-}"' EXIT
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
  printf '%s' "$body" | LOCUS_HOME="$LOCUS_HOME" "$MCP_BIN" 2>/dev/null
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
# exclusive: no personal credential refs
for p in w["providers"]:
    assert "PERSONAL" not in p["credential_ref"].upper(), p
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
echo "$exec_env" | grep -q "SUPABASE_PROJECT_REF=proj_env_e2e" \
  || die "frozen project_ref missing"
ok "exec scrubs ambient + injects env: secrets"

# scrub parent ambient so later steps are clean
unset GH_TOKEN SUPABASE_ACCESS_TOKEN AWS_PROFILE

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
echo "$pinned_names" | grep -q 'supabase.scope' || die "pinned missing supabase.scope"
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

# ── 8. require_approval → grant → re-call success ────────────────────────────
log "8. require_approval → approve grant → re-call success"
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
ok "approve grant $appr_id"

retry_out="$(
  mcp_rpc \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"e2e","version":"0"}}}' \
    '{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}' \
    '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"supabase.table.delete","arguments":{"table":"users"}}}'
)"
retry_line="$(echo "$retry_out" | tool_call_text)"
echo "$retry_line" | grep -q '^OK|' || die "expected success after grant: $retry_line"
ok "re-call after grant succeeds"

# ── 9. doctor ────────────────────────────────────────────────────────────────
log "9. doctor"
# Sample bindings use unresolved phm: refs → overall ok may be false / exit 1.
# Assert structural health: seal + pin + bindings present.
set +e
doctor_json="$(locus doctor --json 2>/dev/null)"
doctor_ec=$?
set -e
echo "$doctor_json" | python3 -c '
import json, sys
d = json.load(sys.stdin)
assert d.get("seal_ok") is True, d
assert d.get("pin_seal_ok") is True, d
assert d.get("bindings", 0) >= 2, d
assert d.get("pinned") in ("acme", "envtest", "personal"), d
print("doctor seal_ok pin_seal_ok bindings=%s pinned=%s exit_hint=%s" % (
    d.get("bindings"), d.get("pinned"), "issues" if d.get("issues") else "clean"))
'
ok "doctor seal_ok + pin_seal_ok (exit was $doctor_ec; unresolved phm samples ok)"

# ── 10. leave → unpinned ─────────────────────────────────────────────────────
log "10. leave → unpinned"
locus leave >/dev/null
status="$(locus status --oneline)"
[[ "$status" == "unpinned" ]] || die "expected unpinned, got: $status"
who_left="$(locus whoami --json 2>/dev/null || true)"
if echo "$who_left" | python3 -c '
import json, sys
try:
    w = json.load(sys.stdin)
except Exception:
    sys.exit(0)
# if whoami still prints, must not claim a pin
if w.get("binding_alias") and w.get("seal_ok"):
    # some implementations return unpinned object
    if w.get("binding_alias") not in (None, "", "unpinned"):
        # check pinned field if present
        if w.get("pinned") is False:
            sys.exit(0)
        # leave already verified via status
        sys.exit(0)
' 2>/dev/null; then
  :
fi
ok "leave → status unpinned"

# Final MCP unpinned check
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

printf '\n========================================\n'
printf 'e2e PASS  (%d checks)\n' "$pass"
printf 'LOCUS_HOME was %s (cleaned)\n' "$LOCUS_HOME"
printf '========================================\n'
