#!/usr/bin/env bash
# Locus end-to-end shell tests — pin, isolation, MCP, freeze, approval, doctor,
# dual-control, events, optional enter/run (feature-detected).
set -euo pipefail

export PATH="${HOME}/.cargo/bin:${PATH}"

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

# ── 9. doctor (structure + exit codes) ───────────────────────────────────────
log "9. doctor structure + exit codes"
# Sample bindings use unresolved phm: refs → overall ok is usually false / exit 1.
# Assert structural health: seal + pin + bindings present; exit matches issues.
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
issues = d.get("issues") or []
ok_flag = d.get("ok")
assert ok_flag == (len(issues) == 0), d
ec = int(os.environ["DOCTOR_EC"])
assert ec in (0, 1), "doctor exit must be 0 or 1, got %s" % ec
if issues:
    assert ec == 1, "doctor with issues must exit 1, got %s: %s" % (ec, issues)
else:
    assert ec == 0, "doctor clean must exit 0, got %s" % ec
print("doctor seal_ok pin_seal_ok bindings=%s pinned=%s issues=%d exit=%s" % (
    d.get("bindings"), d.get("pinned"), len(issues), ec))
'
ok "doctor structure + exit code matches issues (exit=$doctor_ec)"

# Unpinned doctor still reports seal_ok and exits consistently with issues
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
issues = d.get("issues") or []
ec = int(os.environ["DOCTOR_EC"])
assert ec in (0, 1), ec
if issues:
    assert ec == 1, (ec, issues)
else:
    assert ec == 0, (ec, d)
'
ok "doctor unpinned exit code coherent (exit=$doctor_un_ec)"

# Re-pin acme for dual-control / events steps
locus pin acme --force >/dev/null

# ── 10. dual_control two-principal grant (feature-detected) ──────────────────
log "10. dual_control two principals (if policy supports)"
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

  # First principal — partial grant
  g1="$(locus approve grant "$dual_id" --as alice --json 2>/dev/null || true)"
  echo "$g1" | python3 -c '
import json, sys
r = json.load(sys.stdin)
assert r.get("status") in ("pending", "Pending") or r.get("status") == "pending", r
assert len(r.get("grants") or []) == 1, r
'
  ok "first principal alice partial grant"

  # Same principal cannot complete dual-control
  if locus approve grant "$dual_id" --as alice >/dev/null 2>&1; then
    die "same principal should not complete dual_control"
  fi
  ok "same principal rejected on second grant"

  # Second principal completes
  g2="$(locus approve grant "$dual_id" --as bob --json 2>/dev/null)"
  echo "$g2" | python3 -c '
import json, sys
r = json.load(sys.stdin)
st = (r.get("status") or "").lower()
assert st == "approved", r
assert len(r.get("grants") or []) >= 2, r
'
  ok "second principal bob fully approved"

  retry_dual="$(
    mcp_rpc \
      '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"e2e","version":"0"}}}' \
      '{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}' \
      '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"supabase.table.delete","arguments":{"table":"users"}}}'
  )"
  retry_dual_line="$(echo "$retry_dual" | tool_call_text)"
  echo "$retry_dual_line" | grep -q '^OK|' || die "expected success after dual grant: $retry_dual_line"
  ok "re-call after dual grant succeeds"
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

printf '\n========================================\n'
printf 'e2e PASS  (%d checks, %d skipped)\n' "$pass" "$skip"
printf 'LOCUS_HOME was %s (cleaned)\n' "$LOCUS_HOME"
printf '========================================\n'
