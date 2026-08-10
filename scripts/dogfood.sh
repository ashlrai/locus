#!/usr/bin/env bash
# dogfood.sh — local identity-plane readiness path for Locus dogfood.
#
# Steps:
#   1. locus quickstart
#   2. locus agent setup --client claude (--dry-run by default, --apply with DOGFOOD_APPLY=1)
#   3. locus agent report --json | jq .status
#   4. locus doctor
#   5. locus forensics export --out /tmp/pack.json (or $DOGFOOD_PACK)
#   5b. locus verify session --json (shape + secrets hard; session_ok hard at ready gate)
#   6. locus goal status (northstar progress)
#   7. scripts/hub-smoke.sh (ashlr-hub CLI contract; own throwaway home)
#
# Prints "DOGFOOD READY" only after every required readiness probe is green.
#
# Safe by default: uses a throwaway LOCUS_HOME unless DOGFOOD_USE_REAL_HOME=1.
# Never prints secret values or credential locators.
# `DOGFOOD_SKIP_HUB_SMOKE=1` is diagnostic-only and can never reach READY.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
export PATH="${ROOT}/target/debug:${ROOT}/target/release:${HOME}/.cargo/bin:${PATH}"

log()  { printf '\n==> %s\n' "$*"; }
ok()   { printf '  ok  %s\n' "$*"; }
die()  { printf '  FAIL %s\n' "$*" >&2; exit 1; }

readiness_gate() {
  local report="$1" report_rc="$2" doctor_rc="$3" verify="$4" verify_rc="$5" hub_ok="$6"
  [[ "$report_rc" -eq 0 ]] || return 1
  [[ "$doctor_rc" -eq 0 ]] || return 1
  [[ "$verify_rc" -eq 0 ]] || return 1
  [[ "$hub_ok" -eq 1 ]] || return 1
  printf '%s' "$report" | jq -e '
    .status == "ready"
    and .ready == true
    and .pin != null
    and (.status_oneline | type == "string" and contains(":"))
    and .doctor.verdict == "SAFE"
    and .doctor.ok == true
    and ((.doctor.unresolved_phm // []) | length == 0)
  ' >/dev/null 2>&1 || return 1
  printf '%s' "$report" | jq -e -f "$ROOT/scripts/dogfood-ready.jq" >/dev/null 2>&1 || return 1
  printf '%s' "$verify" | jq -e '
    .kind == "session"
    and .session_ok == true
    and .doctor.verdict == "SAFE"
    and .doctor.ok == true
    and ((.doctor.unresolved_phm // []) | length == 0)
    and .safe_next.ready == true
    and .safe_next.action == "ready"
  ' >/dev/null 2>&1
}

dogfood_gate_self_test() {
  local ready warn unresolved protected verify_ready
  ready='{"status":"ready","ready":true,"pin":{"alias":"a"},"status_oneline":"a:t","required_servers":["locus","phantom"],"mcp_command":"locus-mcp","doctor":{"verdict":"SAFE","ok":true,"unresolved_phm":[],"findings":[]}}'
  warn='{"status":"ready","ready":true,"pin":{"alias":"a"},"status_oneline":"a:t","required_servers":["locus","phantom"],"mcp_command":"locus-mcp","doctor":{"verdict":"WARN","ok":false,"unresolved_phm":[],"findings":[]}}'
  unresolved='{"status":"ready","ready":true,"pin":{"alias":"a"},"status_oneline":"a:t","required_servers":["locus","phantom"],"mcp_command":"locus-mcp","doctor":{"verdict":"SAFE","ok":true,"unresolved_phm":[{"provider":"github"}],"findings":[]}}'
  protected='{"status":"protected","ready":false,"pin":{"alias":"a"},"status_oneline":"a:t","required_servers":["locus","phantom"],"mcp_command":"locus-mcp","doctor":{"verdict":"SAFE","ok":true,"unresolved_phm":[],"findings":[]}}'
  verify_ready='{"kind":"session","session_ok":true,"doctor":{"verdict":"SAFE","ok":true,"unresolved_phm":[]},"safe_next":{"ready":true,"action":"ready"}}'

  readiness_gate "$ready" 0 0 "$verify_ready" 0 1 || die "self-test rejected complete readiness"
  ! readiness_gate "$warn" 0 0 "$verify_ready" 0 1 || die "self-test reproduced WARN false-ready"
  ! readiness_gate "$ready" 0 1 "$verify_ready" 0 1 || die "self-test reproduced nonzero doctor false-ready"
  ! readiness_gate "$unresolved" 0 0 "$verify_ready" 0 1 || die "self-test reproduced unresolved credential false-ready"
  ! readiness_gate "$protected" 1 0 "$verify_ready" 0 1 || die "self-test reproduced protection-only false-ready"
  ! readiness_gate "$ready" 0 0 "$verify_ready" 0 0 || die "self-test reproduced skipped Hub smoke false-ready"
  printf 'dogfood readiness gate self-test: ok\n'
}

if ! command -v jq >/dev/null 2>&1; then
  die "jq is required"
fi

if [[ "${DOGFOOD_SELF_TEST:-0}" == "1" ]]; then
  dogfood_gate_self_test
  exit 0
fi

# Prefer a local build that has agent/forensics/quickstart.
if ! command -v locus >/dev/null 2>&1 || ! locus agent report --help >/dev/null 2>&1; then
  log "building locus-cli…"
  (cd "$ROOT" && cargo build -q -p locus-cli)
fi

USE_REAL="${DOGFOOD_USE_REAL_HOME:-0}"
APPLY="${DOGFOOD_APPLY:-0}"
PACK_OUT="${DOGFOOD_PACK:-/tmp/pack.json}"
CLIENT="${DOGFOOD_CLIENT:-claude}"
SKIP_HUB="${DOGFOOD_SKIP_HUB_SMOKE:-0}"

cleanup() {
  if [[ "${USE_REAL}" != "1" && -n "${DOGFOOD_HOME:-}" && -d "${DOGFOOD_HOME}" ]]; then
    rm -rf "${DOGFOOD_HOME}"
  fi
}
trap cleanup EXIT

if [[ "${USE_REAL}" == "1" ]]; then
  log "using real LOCUS_HOME (${LOCUS_HOME:-~/.locus})"
  unset LOCUS_HOME 2>/dev/null || true
  # Allow caller to set LOCUS_HOME explicitly for real dogfood
  if [[ -n "${DOGFOOD_HOME:-}" ]]; then
    export LOCUS_HOME="$DOGFOOD_HOME"
  fi
else
  DOGFOOD_HOME="$(mktemp -d "${TMPDIR:-/tmp}/locus-dogfood.XXXXXX")"
  export LOCUS_HOME="$DOGFOOD_HOME"
  unset LOCUS_SESSION_ID || true
  log "isolated LOCUS_HOME=$LOCUS_HOME"
fi

echo "locus: $(command -v locus) ($(locus --version 2>/dev/null || true))"

# ── 1. quickstart ────────────────────────────────────────────────────────────
log "1. locus quickstart"
locus quickstart
ok "quickstart"

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

# ── Ready gate ───────────────────────────────────────────────────────────────
log "dogfood gate"
if ! readiness_gate "$REPORT" "$REPORT_RC" "$DOCTOR_RC" "$VS_JSON" "$VS_RC" "$HUB_OK"; then
  die "readiness blocked (status=${STATUS} ready=${READY} pin=${HAS_PIN} oneline=${ONELINE} report_exit=${REPORT_RC} doctor_exit=${DOCTOR_RC} verify_exit=${VS_RC} hub_ok=${HUB_OK})"
fi
ok "strict readiness evidence"
echo "DOGFOOD READY"
