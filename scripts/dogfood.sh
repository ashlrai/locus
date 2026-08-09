#!/usr/bin/env bash
# dogfood.sh — local identity-plane readiness path for Locus dogfood.
#
# Steps:
#   1. locus quickstart
#   2. locus agent setup --client claude (--dry-run by default, --apply with DOGFOOD_APPLY=1)
#   3. locus agent report --json | jq .status
#   4. locus doctor
#   5. locus forensics export --out /tmp/pack.json (or $DOGFOOD_PACK)
#
# Prints "DOGFOOD READY" only when the same fail-closed contract as Hub dispatch passes.
#
# Safe by default: uses a throwaway LOCUS_HOME unless DOGFOOD_USE_REAL_HOME=1.
# Never prints secret values or credential locators.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
export PATH="${ROOT}/target/debug:${ROOT}/target/release:${HOME}/.cargo/bin:${PATH}"

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

# ── Ready gate ───────────────────────────────────────────────────────────────
log "dogfood gate"
if [[ "$REPORT_RC" -ne 0 ]] \
  || [[ "$STATUS" != "ready" ]] \
  || [[ "$READY" != "true" ]] \
  || [[ "$HAS_PIN" != "true" ]] \
  || [[ "$ONELINE" != *:* ]] \
  || ! printf '%s' "$REPORT" | jq -e -f "$ROOT/scripts/dogfood-ready.jq" >/dev/null 2>&1; then
  die "dispatch contract blocked (status=${STATUS} ready=${READY} pin=${HAS_PIN} oneline=${ONELINE} exit=${REPORT_RC})"
fi
echo "DOGFOOD READY"
