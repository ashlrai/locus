#!/usr/bin/env bash
# dogfood.sh — local identity-plane readiness path for Locus dogfood.
#
# Steps:
#   1. locus quickstart
#   1b. spawn a deterministic MCP fixture through the upstream sandbox
#   2. locus agent setup --client claude (applied in isolated mode)
#   3. locus agent report --json | jq .status
#   4. locus doctor
#   5. locus forensics export --out /tmp/pack.json (or $DOGFOOD_PACK)
#   5b. locus verify session --json (shape + secrets hard; session_ok hard at ready gate)
#   6. locus goal status (northstar progress)
#   7. scripts/hub-smoke.sh (ashlr-hub CLI contract; own throwaway home)
#   8. (optional) scripts/dogfood-clients.sh when DOGFOOD_CLIENTS=1 —
#      soft multi-client install probe; never blocks DOGFOOD READY by default
#
# Prints "DOGFOOD READY" only after every required readiness probe is green.
#
# Safe by default: uses a throwaway LOCUS_HOME unless DOGFOOD_USE_REAL_HOME=1.
# Never prints secret values or credential locators.
# `DOGFOOD_SKIP_HUB_SMOKE=1` is diagnostic-only and can never reach READY.
# `DOGFOOD_CLIENTS=1` runs the multi-client probe (soft-skip missing installs).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
export PATH="${ROOT}/target/debug:${ROOT}/target/release:${HOME}/.cargo/bin:${PATH}"
export LOCUS_CONTROL_CAPABILITY="${LOCUS_CONTROL_CAPABILITY:-$(od -An -N32 -tx1 /dev/urandom | tr -d ' \n')}"

log()  { printf '\n==> %s\n' "$*"; }
ok()   { printf '  ok  %s\n' "$*"; }
die()  { printf '  FAIL %s\n' "$*" >&2; exit 1; }

readiness_gate() {
  local report="$1" report_rc="$2" doctor_rc="$3" verify="$4" verify_rc="$5" hub_ok="$6" sandbox_ok="$7"
  [[ "$report_rc" -eq 0 ]] || return 1
  [[ "$doctor_rc" -eq 0 ]] || return 1
  [[ "$verify_rc" -eq 0 ]] || return 1
  [[ "$hub_ok" -eq 1 ]] || return 1
  [[ "$sandbox_ok" -eq 1 ]] || return 1
  jq -ne --argjson report "$report" --argjson verify "$verify" '
    def nonempty: type == "string" and length > 0;
    def runtime_matches($runtime; $pin; $session_id):
      $runtime.pinned == true
      and $runtime.seal_ok == true
      and $runtime.binding_present == true
      and $runtime.binding_id_match == true
      and $runtime.tenant_match == true
      and $runtime.providers_match == true
      and $runtime.frozen == false
      and $runtime.expired == false
      and $runtime.ok == true
      and (($runtime.issues // []) | length == 0)
      and $runtime.session_id == $session_id
      and $runtime.binding_alias == $pin.alias
      and $runtime.binding_id_session == $pin.binding_id
      and $runtime.binding_id_file == $pin.binding_id
      and $runtime.tenant_session == $pin.tenant
      and $runtime.tenant_file == $pin.tenant;
    ($report.pin) as $pin
    | ($verify.whoami) as $who
    | ($who.session_id) as $session_id
    | $report.status == "ready"
    and $report.ready == true
    and $report.exit_code == 0
    and ($pin.alias | nonempty)
    and ($pin.tenant | nonempty)
    and ($pin.binding_id | nonempty)
    and ($pin.expires_at | nonempty)
    and $pin.seal_ok == true
    and $pin.expired == false
    and $report.status_oneline == ($pin.alias + ":" + $pin.tenant)
    and $report.env_session_id == null
    and $report.home == $report.doctor.home
    and $report.doctor.pin == $pin
    and $report.doctor.pin_seal_ok == true
    and $report.doctor.verdict == "SAFE"
    and $report.doctor.ok == true
    and (($report.doctor.unresolved_phm // []) | length == 0)
    and runtime_matches($report.doctor.runtime; $pin; $session_id)
    and $verify.kind == "session"
    and $verify.session_ok == true
    and ($session_id | nonempty)
    and $who.binding_alias == $pin.alias
    and $who.binding_id == $pin.binding_id
    and $who.tenant == $pin.tenant
    and $who.expires_at == $pin.expires_at
    and ($who.worker_home | nonempty)
    and ($who.providers | type == "array" and length > 0)
    and $who.seal_ok == true
    and $who.frozen == false
    and $verify.doctor.home == $report.home
    and $verify.doctor.pin == $pin
    and $verify.doctor.pin_seal_ok == true
    and $verify.doctor.verdict == "SAFE"
    and $verify.doctor.ok == true
    and (($verify.doctor.unresolved_phm // []) | length == 0)
    and $verify.doctor.runtime == $report.doctor.runtime
    and runtime_matches($verify.doctor.runtime; $pin; $session_id)
    and $verify.doctor.runtime.providers == $who.providers
    and $verify.safe_next.ready == true
    and $verify.safe_next.action == "ready"
    and $verify.safe_next.binding == $pin.alias
    and $verify.safe_next.tenant == $pin.tenant
  ' >/dev/null 2>&1 || return 1
  printf '%s' "$report" | jq -e -f "$ROOT/scripts/dogfood-ready.jq" >/dev/null 2>&1 || return 1
  return 0
}

dogfood_gate_self_test() {
  local ready warn unresolved protected verify_ready verify_not_ready report_identity_bad verify_identity_bad session_bad env_override
  local pin runtime doctor whoami
  pin='{"alias":"dogfood","tenant":"dogfood","binding_id":"bnd_dogfood","expires_at":"2026-08-10T12:00:00Z","seal_ok":true,"expired":false}'
  runtime='{"pinned":true,"seal_ok":true,"binding_present":true,"binding_id_match":true,"tenant_match":true,"providers_match":true,"frozen":false,"expired":false,"session_id":"ses_abc","binding_alias":"dogfood","binding_id_session":"bnd_dogfood","binding_id_file":"bnd_dogfood","tenant_session":"dogfood","tenant_file":"dogfood","providers":[{"provider":"github","account":"dogfood","credential":{"present":true,"source":"env"},"project_ref":null,"team_id":null,"account_id":null,"read_only":null,"orgs":["dogfood"],"repos":[]}],"issues":[],"ok":true}'
  doctor="$(jq -cn --argjson pin "$pin" --argjson runtime "$runtime" '{home:"/tmp/locus",pin:$pin,pin_seal_ok:true,runtime:$runtime,verdict:"SAFE",ok:true,unresolved_phm:[]}')"
  whoami='{"session_id":"ses_abc","binding_alias":"dogfood","binding_id":"bnd_dogfood","tenant":"dogfood","principal":null,"providers":[{"provider":"github","account":"dogfood","credential":{"present":true,"source":"env"},"project_ref":null,"team_id":null,"account_id":null,"read_only":null,"orgs":["dogfood"],"repos":[]}],"expires_at":"2026-08-10T12:00:00Z","worker_home":"/tmp/locus/workers/ses_abc","seal_ok":true,"frozen":false,"mode":"exclusive","namespaces":[]}'
  ready="$(jq -cn --argjson pin "$pin" --argjson doctor "$doctor" '{status:"ready",ready:true,exit_code:0,pin:$pin,status_oneline:"dogfood:dogfood",home:"/tmp/locus",required_servers:["locus","phantom"],mcp_command:"locus-mcp",doctor:$doctor}')"
  verify_ready="$(jq -cn --argjson whoami "$whoami" --argjson doctor "$doctor" '{kind:"session",session_ok:true,whoami:$whoami,doctor:$doctor,safe_next:{ready:true,action:"ready",binding:"dogfood",tenant:"dogfood"}}')"
  warn="$(printf '%s' "$ready" | jq '.doctor.verdict = "WARN" | .doctor.ok = false')"
  unresolved="$(printf '%s' "$ready" | jq '.doctor.unresolved_phm = [{"provider":"github"}]')"
  protected="$(printf '%s' "$ready" | jq '.status = "protected" | .ready = false | .exit_code = 1')"
  verify_not_ready="$(printf '%s' "$verify_ready" | jq '.session_ok = false')"
  report_identity_bad="$(printf '%s' "$ready" | jq '.doctor.runtime.binding_id_file = "bnd_other"')"
  verify_identity_bad="$(printf '%s' "$verify_ready" | jq '.whoami.binding_id = "bnd_other"')"
  session_bad="$(printf '%s' "$verify_ready" | jq '.doctor.runtime.session_id = "ses_other"')"
  env_override="$(printf '%s' "$ready" | jq '.env_session_id = "ses_stale"')"

  readiness_gate "$ready" 0 0 "$verify_ready" 0 1 1 || die "self-test rejected complete readiness"
  ! readiness_gate "$warn" 0 0 "$verify_ready" 0 1 1 || die "self-test reproduced WARN false-ready"
  ! readiness_gate "$ready" 0 1 "$verify_ready" 0 1 1 || die "self-test reproduced nonzero doctor false-ready"
  ! readiness_gate "$unresolved" 0 0 "$verify_ready" 0 1 1 || die "self-test reproduced unresolved credential false-ready"
  ! readiness_gate "$protected" 1 0 "$verify_ready" 0 1 1 || die "self-test reproduced protection-only false-ready"
  ! readiness_gate "$ready" 0 0 "$verify_ready" 0 0 1 || die "self-test reproduced skipped Hub smoke false-ready"
  ! readiness_gate "$ready" 0 0 "$verify_not_ready" 0 1 1 || die "self-test accepted session_ok=false"
  ! readiness_gate "$ready" 0 0 "$verify_ready" 1 1 1 || die "self-test accepted nonzero verify-session exit"
  ! readiness_gate "$report_identity_bad" 0 0 "$verify_ready" 0 1 1 || die "self-test accepted report binding mismatch"
  ! readiness_gate "$ready" 0 0 "$verify_identity_bad" 0 1 1 || die "self-test accepted whoami binding mismatch"
  ! readiness_gate "$ready" 0 0 "$session_bad" 0 1 1 || die "self-test accepted doctor/whoami session mismatch"
  ! readiness_gate "$env_override" 0 0 "$verify_ready" 0 1 1 || die "self-test accepted env-selected identity"
  ! readiness_gate "$ready" 0 0 "$verify_ready" 0 1 0 || die "self-test accepted missing sandbox attestation"
  printf 'dogfood readiness gate self-test: ok\n'
}

sandbox_attestation() {
  local probe_root
  probe_root="$(mktemp -d "${TMPDIR:-/tmp}/locus-sandbox-dogfood.XXXXXX")"

  (
    trap 'rm -rf "$probe_root"' EXIT
    export LOCUS_HOME="$probe_root/home"
    unset LOCUS_SESSION_ID LOCUS_DOGFOOD_SANDBOX_UNUSED || true

    local project="$probe_root/project"
    local fixture="$project/sandbox-marker-mcp"
    local binding_file="$LOCUS_HOME/bindings/sandbox-probe.toml"
    local stdout_file="$probe_root/mcp.stdout"
    local stderr_file="$probe_root/mcp.stderr"
    mkdir -p "$project"

    # Shell builtins only: the fixture performs no credential lookup or network I/O.
    printf '%s\n' \
      '#!/bin/sh' \
      'while IFS= read -r line; do' \
      '  case "$line" in' \
      '    *'\''"method":"initialize"'\''*)' \
      '      printf '\''%s\n'\'' '\''{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"locus-sandbox-marker","version":"1"}}}'\''' \
      '      ;;' \
      '    *'\''"method":"tools/list"'\''*)' \
      '      printf '\''%s\n'\'' '\''{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"sandbox_attest","description":"Return applied sandbox markers","inputSchema":{"type":"object"}}]}}'\''' \
      '      ;;' \
      '    *'\''"method":"tools/call"'\''*)' \
      '      printf '\''{"jsonrpc":"2.0","id":3,"result":{"content":[{"type":"text","text":"sandboxed=%s backend=%s"}],"isError":false}}\n'\'' "${LOCUS_WORKER_SANDBOXED:-unset}" "${LOCUS_WORKER_SANDBOX_BACKEND:-unset}"' \
      '      ;;' \
      '  esac' \
      'done' >"$fixture"
    chmod 700 "$fixture"

    cd "$project"
    locus init >/dev/null
    locus binding add sandbox-probe \
      --tenant sandbox-probe \
      --provider github \
      --account sandbox-probe \
      --credential-ref env:LOCUS_DOGFOOD_SANDBOX_UNUSED \
      --org sandbox-probe >/dev/null
    [[ -f "$binding_file" ]] || {
      printf '  sandbox probe binding missing at %s\n' "$binding_file" >&2
      exit 1
    }
    local fixture_toml
    fixture_toml="$(jq -Rn --arg value "$fixture" '$value')"
    printf '\n[binding.providers.upstream]\ncommand = %s\nresolve_secrets = false\nsandbox = true\n' \
      "$fixture_toml" >>"$binding_file"
    locus pin sandbox-probe >/dev/null

    local rpc_out rpc_rc
    set +e
    printf '%s\n' \
      '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"dogfood","version":"1"}}}' \
      '{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}' \
      '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}' \
      '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"github.sandbox_attest","arguments":{}}}' \
      | locus exec --no-resolve -- "$MCP_BIN" >"$stdout_file" 2>"$stderr_file"
    rpc_rc=$?
    set -e
    rpc_out="$(cat "$stdout_file")"

    [[ "$rpc_rc" -eq 0 ]] || {
      printf '  sandbox probe transport failed (exit=%s): %s\n' "$rpc_rc" "$(cat "$stderr_file")" >&2
      exit 1
    }
    if grep -Fq 'LOCUS_DOGFOOD_SANDBOX_UNUSED' "$stdout_file" "$stderr_file"; then
      printf '  sandbox probe leaked a credential locator\n' >&2
      exit 1
    fi

    if [[ "$(uname -s)" != "Darwin" ]]; then
      if printf '%s\n' "$rpc_out" | jq -se '
        (map(select(.id == 2))[0].result.tools | map(.name) | index("github.sandbox_attest") | not)
        and (map(select(.id == 3))[0].result.isError == true)
        and ((map(select(.id == 3))[0].result.content[0].text | fromjson | .detail) | contains("no supported OS isolation backend"))
      ' >/dev/null; then
        printf '  sandbox backend unsupported; fail-closed spawn refusal attested\n' >&2
        exit 2
      fi
      printf '  unsupported platform did not return the required fail-closed refusal\n' >&2
      cat "$stderr_file" >&2
      exit 1
    fi

    printf '%s\n' "$rpc_out" | jq -se '
      (map(select(.id == 2))[0].result.tools | map(.name) | index("github.sandbox_attest") | not)
      and (map(select(.id == 3))[0].result.isError == false)
      and ((map(select(.id == 3))[0].result.content[0].text | fromjson)
        | .isError == false
        and .content[0].text == "sandboxed=1 backend=sandbox-exec")
    ' >/dev/null || {
      printf '  sandbox marker/backend attestation failed\n' >&2
      cat "$stdout_file" >&2
      cat "$stderr_file" >&2
      exit 1
    }
  )
}

if ! command -v jq >/dev/null 2>&1; then
  die "jq is required"
fi

if [[ "${DOGFOOD_SELF_TEST:-0}" == "1" ]]; then
  dogfood_gate_self_test
  exit 0
fi

# DOGFOOD READY attests the checked-out source, never a stale installed binary.
command -v cargo >/dev/null 2>&1 || die "cargo is required to build exact dogfood binaries"
log "building exact workspace binaries"
(cd "$ROOT" && cargo build -q -p locus-cli -p locus-mcp)
LOCUS_BIN="$ROOT/target/debug/locus"
MCP_BIN="$ROOT/target/debug/locus-mcp"
[[ -x "$LOCUS_BIN" ]] || die "missing exact locus binary at $LOCUS_BIN"
[[ -x "$MCP_BIN" ]] || die "missing exact locus-mcp binary at $MCP_BIN"
locus() { "$LOCUS_BIN" "$@"; }

USE_REAL="${DOGFOOD_USE_REAL_HOME:-0}"
APPLY="${DOGFOOD_APPLY:-}"
PACK_OUT="${DOGFOOD_PACK:-}"
CLIENT="${DOGFOOD_CLIENT:-claude}"
SKIP_HUB="${DOGFOOD_SKIP_HUB_SMOKE:-0}"
RUN_CLIENTS="${DOGFOOD_CLIENTS:-0}"

cleanup() {
  if [[ "${USE_REAL}" != "1" && -n "${DOGFOOD_HOME:-}" && -d "${DOGFOOD_HOME}" ]]; then
    rm -rf "${DOGFOOD_HOME}"
  fi
}
trap cleanup EXIT

if [[ "${USE_REAL}" == "1" ]]; then
  APPLY="${APPLY:-0}"
  PACK_OUT="${PACK_OUT:-/tmp/pack.json}"
  log "using real LOCUS_HOME (${LOCUS_HOME:-~/.locus})"
  unset LOCUS_HOME 2>/dev/null || true
  # Allow caller to set LOCUS_HOME explicitly for real dogfood
  if [[ -n "${DOGFOOD_HOME:-}" ]]; then
    export LOCUS_HOME="$DOGFOOD_HOME"
  fi
else
  APPLY="${APPLY:-1}"
  DOGFOOD_HOME="$(mktemp -d "${TMPDIR:-/tmp}/locus-dogfood.XXXXXX")"
  export LOCUS_HOME="$DOGFOOD_HOME"
  PACK_OUT="${PACK_OUT:-$DOGFOOD_HOME/pack.json}"
  unset LOCUS_SESSION_ID || true
  log "isolated LOCUS_HOME=$LOCUS_HOME"
  DOGFOOD_PROJECT="$DOGFOOD_HOME/project"
  mkdir -p "$DOGFOOD_PROJECT"
  cd "$DOGFOOD_PROJECT"
  export LOCUS_DOGFOOD_TOKEN="dogfood-local-${PPID}-${RANDOM}"
  locus init >/dev/null
  locus binding add dogfood \
    --tenant dogfood \
    --provider github \
    --account dogfood \
    --credential-ref env:LOCUS_DOGFOOD_TOKEN \
    --org dogfood >/dev/null
fi

echo "locus: $LOCUS_BIN ($(locus --version 2>/dev/null || true))"

# ── 1. quickstart ────────────────────────────────────────────────────────────
log "1. locus quickstart"
locus quickstart
ok "quickstart"

# ── 1b. upstream worker sandbox attestation ──────────────────────────────────
log "1b. upstream worker sandbox attestation"
SANDBOX_OK=0
set +e
sandbox_attestation
SANDBOX_RC=$?
set -e
case "$SANDBOX_RC" in
  0)
    SANDBOX_OK=1
    ok "upstream fixture sandboxed=1 backend=sandbox-exec"
    ;;
  2)
    die "sandbox backend unsupported; refusal attested and DOGFOOD READY is unavailable"
    ;;
  *)
    die "upstream sandbox attestation failed"
    ;;
esac

# ── 2. agent setup (dry-run or apply) ────────────────────────────────────────
log "2. locus agent setup --client ${CLIENT}"
if [[ "${APPLY}" == "1" ]]; then
  locus agent setup --apply --client "${CLIENT}"
  ok "agent setup --apply --client ${CLIENT}"
else
  locus agent setup --dry-run --client "${CLIENT}"
  ok "agent setup --dry-run --client ${CLIENT} (set DOGFOOD_APPLY=1 to apply)"
fi

# ── 3. agent report ──────────────────────────────────────────────────────────
log "3. locus agent report --json"
set +e
REPORT="$(locus agent report --json 2>/dev/null)"
REPORT_RC=$?
set -e

if ! printf '%s' "$REPORT" | jq -e . >/dev/null 2>&1; then
  die "agent report did not emit JSON (exit=${REPORT_RC})"
fi

# Secret and credential-locator hygiene
if printf '%s' "$REPORT" | grep -EEq 'ghp_|sk-[a-zA-Z0-9]{10,}|xox[baprs]-|"credential_ref"|phm:|env:|test:'; then
  die "possible secret or credential locator material in agent report JSON"
fi

STATUS="$(printf '%s' "$REPORT" | jq -r '.status')"
READY="$(printf '%s' "$REPORT" | jq -r '.ready')"
HAS_PIN="$(printf '%s' "$REPORT" | jq -r 'if .pin != null then "true" else "false" end')"
ONELINE="$(printf '%s' "$REPORT" | jq -r '.status_oneline')"
printf '  status=%s ready=%s pin=%s oneline=%s exit=%s\n' \
  "$STATUS" "$READY" "$HAS_PIN" "$ONELINE" "$REPORT_RC"
ok "agent report --json"

# ── 4. doctor ────────────────────────────────────────────────────────────────
log "4. locus doctor"
set +e
locus doctor
DOCTOR_RC=$?
set -e
ok "doctor (exit=${DOCTOR_RC})"

# ── 5. forensics export ──────────────────────────────────────────────────────
log "5. locus forensics export --out ${PACK_OUT}"
locus forensics export --out "${PACK_OUT}"
[[ -f "${PACK_OUT}" ]] || die "forensics pack missing at ${PACK_OUT}"
if ! jq -e . "${PACK_OUT}" >/dev/null 2>&1; then
  die "forensics pack is not valid JSON"
fi
# Never allow secrets or credential locators in the pack
if grep -EEq 'ghp_|sk-[a-zA-Z0-9]{10,}|xox[baprs]-|"credential_ref"|phm:|env:|test:' "${PACK_OUT}"; then
  die "possible secret or credential locator material in forensics pack"
fi
ok "forensics pack → ${PACK_OUT} ($(wc -c <"${PACK_OUT}" | tr -d ' ') bytes)"

# ── 5b. verify session (required readiness evidence) ─────────────────────────
log "5b. locus verify session --json"
if ! locus verify session --help >/dev/null 2>&1; then
  die "verify session command is required"
fi
set +e
VS_JSON="$(locus verify session --json 2>/dev/null)"
VS_RC=$?
set -e
if ! printf '%s' "$VS_JSON" | jq -e . >/dev/null 2>&1; then
  die "verify session did not emit JSON (exit=${VS_RC})"
fi
if printf '%s' "$VS_JSON" | grep -EEq 'ghp_|sk-[a-zA-Z0-9]{10,}|xox[baprs]-|github_pat_|AKIA|secret_value'; then
  die "possible secret material in verify session JSON"
fi
KIND="$(printf '%s' "$VS_JSON" | jq -r '.kind // empty')"
SESSION_OK="$(printf '%s' "$VS_JSON" | jq -r 'if .session_ok == true then "true" elif .session_ok == false then "false" else empty end')"
HAS_DOCTOR="$(printf '%s' "$VS_JSON" | jq -r 'if .doctor != null then "true" else "false" end')"
HAS_SAFE_NEXT="$(printf '%s' "$VS_JSON" | jq -r 'if .safe_next != null then "true" else "false" end')"
if [[ "$KIND" != "session" ]] \
  || [[ -z "$SESSION_OK" ]] \
  || [[ "$HAS_DOCTOR" != "true" ]] \
  || [[ "$HAS_SAFE_NEXT" != "true" ]]; then
  die "verify session pack shape invalid (kind=${KIND} session_ok=${SESSION_OK} doctor=${HAS_DOCTOR} safe_next=${HAS_SAFE_NEXT})"
fi
printf '  kind=%s session_ok=%s doctor=%s safe_next=%s exit=%s\n' \
  "$KIND" "$SESSION_OK" "$HAS_DOCTOR" "$HAS_SAFE_NEXT" "$VS_RC"
ok "verify session --json"

# ── 6. goal status (northstar; walk GOALS.md from repo root) ──────────────────
log "6. locus goal status"
if locus goal status --help >/dev/null 2>&1; then
  # Prefer repo GOALS.md when dogfood is run from a checkout
  set +e
  (
    cd "$ROOT"
    locus goal status
  )
  GOAL_RC=$?
  set -e
  set +e
  GOAL_JSON="$(
    cd "$ROOT"
    locus goal status --json 2>/dev/null
  )"
  set -e
  if printf '%s' "$GOAL_JSON" | jq -e . >/dev/null 2>&1; then
    DONE="$(printf '%s' "$GOAL_JSON" | jq -r '
      if .milestones then ([.milestones[].done] | add // 0)
      elif .done then .done
      else "?" end')"
    TOTAL="$(printf '%s' "$GOAL_JSON" | jq -r '
      if .milestones then ([.milestones[].total] | add // 0)
      elif .total then .total
      else "?" end')"
    printf '  goal progress: %s / %s done (exit=%s)\n' "$DONE" "$TOTAL" "$GOAL_RC"
  else
    printf '  goal status text mode (exit=%s)\n' "$GOAL_RC"
  fi
  ok "goal status"
else
  printf '  skip goal status (command not available)\n'
fi

# ── 7. hub-smoke (own LOCUS_HOME; hub CLI contract) ──────────────────────────
log "7. scripts/hub-smoke.sh"
HUB_OK=0
if [[ "${SKIP_HUB}" == "1" ]]; then
  printf '  skip hub-smoke (diagnostic only; readiness will fail)\n'
elif [[ ! -x "$ROOT/scripts/hub-smoke.sh" && ! -f "$ROOT/scripts/hub-smoke.sh" ]]; then
  die "hub-smoke.sh missing at $ROOT/scripts/hub-smoke.sh"
else
  # hub-smoke is self-contained (own throwaway home); do not pollute dogfood pin.
  bash "$ROOT/scripts/hub-smoke.sh"
  HUB_OK=1
  ok "hub-smoke"
fi

# ── 8. optional multi-client install probe (soft; does not gate READY) ───────
if [[ "${RUN_CLIENTS}" == "1" ]]; then
  log "8. scripts/dogfood-clients.sh (DOGFOOD_CLIENTS=1)"
  if [[ ! -f "$ROOT/scripts/dogfood-clients.sh" ]]; then
    die "dogfood-clients.sh missing at $ROOT/scripts/dogfood-clients.sh"
  fi
  # Soft by default: missing installs exit 0. Hard-fail only when the operator
  # sets LOCUS_DOGFOOD_REQUIRE_CLIENTS=1 (or setup fails for a found client).
  bash "$ROOT/scripts/dogfood-clients.sh"
  ok "multi-client probe"
else
  printf '\n==> 8. multi-client probe skipped (set DOGFOOD_CLIENTS=1 to run)\n'
fi

# ── Ready gate ───────────────────────────────────────────────────────────────
log "dogfood gate"
if ! readiness_gate "$REPORT" "$REPORT_RC" "$DOCTOR_RC" "$VS_JSON" "$VS_RC" "$HUB_OK" "$SANDBOX_OK"; then
  die "readiness blocked (status=${STATUS} ready=${READY} pin=${HAS_PIN} oneline=${ONELINE} report_exit=${REPORT_RC} doctor_exit=${DOCTOR_RC} verify_exit=${VS_RC} hub_ok=${HUB_OK} sandbox_ok=${SANDBOX_OK})"
fi
ok "strict readiness evidence"
echo "DOGFOOD READY"
