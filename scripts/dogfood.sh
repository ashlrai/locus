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
# Prints "DOGFOOD READY" when identity is pin-safe: status=ready, or protected+pin
# (throwaway / dry-run without full MCP apply). unsafe / unpinned always fail.
#
# Safe by default: uses a throwaway LOCUS_HOME unless DOGFOOD_USE_REAL_HOME=1.
# Never prints secret values or credential locators.
# Skip hub-smoke with DOGFOOD_SKIP_HUB_SMOKE=1.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
export PATH="${ROOT}/target/debug:${ROOT}/target/release:${HOME}/.cargo/bin:${PATH}"
export LOCUS_CONTROL_CAPABILITY="${LOCUS_CONTROL_CAPABILITY:-$(od -An -N32 -tx1 /dev/urandom | tr -d ' \n')}"

log()  { printf '\n==> %s\n' "$*"; }
ok()   { printf '  ok  %s\n' "$*"; }
die()  { printf '  FAIL %s\n' "$*" >&2; exit 1; }

if ! command -v jq >/dev/null 2>&1; then
  die "jq is required"
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

# ── 5b. verify session (structure hard; session_ok enforced at ready gate) ───
# Throwaway quickstart leaves a pin. Mid-path we require a clean JSON pack;
# session_ok is hard-checked when claiming DOGFOOD READY (below).
log "5b. locus verify session --json"
SESSION_OK=""
if locus verify session --help >/dev/null 2>&1; then
  set +e
  VS_JSON="$(locus verify session --json 2>/dev/null)"
  VS_RC=$?
  set -e
  if [[ $VS_RC -ne 0 ]] || ! printf '%s' "$VS_JSON" | jq -e . >/dev/null 2>&1; then
    printf '  warn verify session did not emit JSON (exit=%s) — continuing\n' "${VS_RC}"
  else
    # Secret-value hygiene (mirror e2e claim/session checks; CredentialRef names may appear)
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
    if [[ "$SESSION_OK" != "true" ]]; then
      # Soft mid-path: partial identity can yield session_ok=false before the gate.
      printf '  warn session_ok=false mid-dogfood (will hard-fail at ready gate if still not ok)\n'
    fi
    printf '  kind=%s session_ok=%s doctor=%s safe_next=%s\n' \
      "$KIND" "$SESSION_OK" "$HAS_DOCTOR" "$HAS_SAFE_NEXT"
    ok "verify session --json"
  fi
else
  printf '  skip verify session (command not available)\n'
fi

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
if [[ "${SKIP_HUB}" == "1" ]]; then
  printf '  skip hub-smoke (DOGFOOD_SKIP_HUB_SMOKE=1)\n'
elif [[ ! -x "$ROOT/scripts/hub-smoke.sh" && ! -f "$ROOT/scripts/hub-smoke.sh" ]]; then
  die "hub-smoke.sh missing at $ROOT/scripts/hub-smoke.sh"
else
  # hub-smoke is self-contained (own throwaway home); do not pollute dogfood pin.
  bash "$ROOT/scripts/hub-smoke.sh"
  ok "hub-smoke"
fi

# ── Ready gate ───────────────────────────────────────────────────────────────
# Identity-plane dogfood: accept status=ready (hub mutate gate) OR
# protected+pin (throwaway quickstart without --apply MCP).
# unsafe / unpinned / invalid oneline always fail closed.
log "dogfood gate"
GATE_OK=0
if [[ "$HAS_PIN" == "true" ]] \
  && [[ "$ONELINE" == *:* ]] \
  && [[ "$STATUS" != "unsafe" ]] \
  && printf '%s' "$REPORT" | jq -e -f "$ROOT/scripts/dogfood-ready.jq" >/dev/null 2>&1; then
  if [[ "$STATUS" == "ready" && "$READY" == "true" && "$REPORT_RC" -eq 0 ]]; then
    GATE_OK=1
  elif [[ "$STATUS" == "protected" ]]; then
    # protected+pin is the throwaway / dry-run path (GOALS M3 dogfood contract)
    GATE_OK=1
  fi
fi
if [[ "$GATE_OK" -ne 1 ]]; then
  die "dispatch contract blocked (status=${STATUS} ready=${READY} pin=${HAS_PIN} oneline=${ONELINE} exit=${REPORT_RC})"
fi

# Pinned identity path: hard-require verify session session_ok when CLI is present.
# Throwaway samples often leave agent report protected; session_ok still tracks
# doctor.ok ∧ safe_next.ready for the identity plane.
if locus verify session --help >/dev/null 2>&1; then
  set +e
  VS_GATE="$(locus verify session --json 2>/dev/null)"
  VS_GATE_RC=$?
  set -e
  if [[ $VS_GATE_RC -ne 0 ]] || ! printf '%s' "$VS_GATE" | jq -e . >/dev/null 2>&1; then
    die "verify session failed at ready gate (exit=${VS_GATE_RC})"
  fi
  if printf '%s' "$VS_GATE" | grep -EEq 'ghp_|sk-[a-zA-Z0-9]{10,}|xox[baprs]-|github_pat_|AKIA|secret_value'; then
    die "possible secret material in verify session JSON at ready gate"
  fi
  GATE_SESSION_OK="$(printf '%s' "$VS_GATE" | jq -r 'if .session_ok == true then "true" else "false" end')"
  GATE_KIND="$(printf '%s' "$VS_GATE" | jq -r '.kind // empty')"
  if [[ "$GATE_KIND" != "session" ]] || [[ "$GATE_SESSION_OK" != "true" ]]; then
    die "verify session not ok at ready gate (kind=${GATE_KIND} session_ok=${GATE_SESSION_OK})"
  fi
  ok "verify session session_ok=true at ready gate"
fi
echo "DOGFOOD READY"
