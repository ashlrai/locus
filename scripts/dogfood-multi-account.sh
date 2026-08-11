#!/usr/bin/env bash
# dogfood-multi-account.sh — walk personal + client pins for multi-account dogfood.
#
# For each alias (personal, then client):
#   enter → doctor → verify session --json → agent report --json ready/gate → leave
#
# Requires LOCUS_PERSONAL_ALIAS + LOCUS_CLIENT_ALIAS (or positional args).
# Soft-skip (exit 0 + summary) when either alias is missing/unset.
# Hard-fail when LOCUS_DOGFOOD_REQUIRE_MULTI=1 and aliases missing or a walk fails.
#
# Never prints secret values or credential locators. Never mutates IDE configs.
#
# Usage:
#   LOCUS_PERSONAL_ALIAS=personal LOCUS_CLIENT_ALIAS=client-a scripts/dogfood-multi-account.sh
#   scripts/dogfood-multi-account.sh personal client-a
#   LOCUS_DOGFOOD_REQUIRE_MULTI=1 scripts/dogfood-multi-account.sh personal client-a
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
export PATH="${ROOT}/target/debug:${ROOT}/target/release:${HOME}/.cargo/bin:${PATH}"
export LOCUS_CONTROL_CAPABILITY="${LOCUS_CONTROL_CAPABILITY:-$(od -An -N32 -tx1 /dev/urandom | tr -d ' \n')}"

PERSONAL="${1:-${LOCUS_PERSONAL_ALIAS:-}}"
CLIENT="${2:-${LOCUS_CLIENT_ALIAS:-}}"
REQUIRE="${LOCUS_DOGFOOD_REQUIRE_MULTI:-0}"

log()  { printf '\n==> %s\n' "$*"; }
ok()   { printf '  ok  %s\n' "$*"; }
skip() { printf '  skip %s\n' "$*"; }
warn() { printf '  warn %s\n' "$*" >&2; }
die()  { printf '  FAIL %s\n' "$*" >&2; exit 1; }

# Defensive scrub if anything secret-shaped ever lands in command output.
scrub_secrets() {
  # shellcheck disable=SC2001
  sed -E \
    -e 's/ghp_[A-Za-z0-9_]{10,}/[REDACTED]/g' \
    -e 's/github_pat_[A-Za-z0-9_]{10,}/[REDACTED]/g' \
    -e 's/sk-[A-Za-z0-9]{10,}/[REDACTED]/g' \
    -e 's/xox[baprs]-[A-Za-z0-9-]{10,}/[REDACTED]/g' \
    -e 's/phm_[A-Za-z0-9_]{8,}/[REDACTED]/g' \
    -e 's/(AKIA)[A-Z0-9]{12,}/\1[REDACTED]/g'
}

assert_no_secret_material() {
  local label="$1" body="$2"
  if printf '%s' "$body" | grep -EEq \
    'ghp_|sk-[a-zA-Z0-9]{10,}|xox[baprs]-|github_pat_|AKIA|secret_value|"credential_ref"|phm:|env:|test:'; then
    die "possible secret or credential locator material in ${label}"
  fi
}

ensure_tools() {
  command -v jq >/dev/null 2>&1 || die "jq is required"
  if command -v locus >/dev/null 2>&1; then
    return 0
  fi
  command -v cargo >/dev/null 2>&1 || die "locus not on PATH and cargo missing"
  log "building locus-cli"
  (cd "$ROOT" && cargo build -q -p locus-cli)
  export PATH="${ROOT}/target/debug:${PATH}"
  command -v locus >/dev/null 2>&1 || die "locus binary missing after build"
}

binding_exists() {
  local alias="$1"
  local list
  set +e
  list="$(locus binding list --json 2>/dev/null)"
  set -e
  if ! printf '%s' "$list" | jq -e . >/dev/null 2>&1; then
    return 1
  fi
  printf '%s' "$list" | jq -e --arg a "$alias" '
    (type == "array") and (map(.alias) | index($a) != null)
  ' >/dev/null 2>&1
}

# Ensure leave never leaves a residual pin; ignore "already clear".
safe_leave() {
  set +e
  locus leave >/dev/null 2>&1
  set -e
  return 0
}

# Gate agent report + verify session for one expected alias.
# Prints a short non-secret summary line.
gate_alias() {
  local alias="$1"
  local report report_rc vs vs_rc doctor_rc

  log "enter ${alias}"
  locus enter "$alias" >/dev/null
  ok "entered ${alias}"

  log "doctor (${alias})"
  set +e
  locus doctor >/dev/null
  doctor_rc=$?
  set -e
  # doctor exit: 0 SAFE | 1 WARN | 2 UNSAFE — WARN may still block session_ok
  if [[ "$doctor_rc" -ge 2 ]]; then
    safe_leave
    die "doctor UNSAFE under pin ${alias} (exit=${doctor_rc})"
  fi
  ok "doctor (exit=${doctor_rc})"

  log "verify session --json (${alias})"
  set +e
  vs="$(locus verify session --json 2>/dev/null)"
  vs_rc=$?
  set -e
  if ! printf '%s' "$vs" | jq -e . >/dev/null 2>&1; then
    safe_leave
    die "verify session did not emit JSON for ${alias} (exit=${vs_rc})"
  fi
  assert_no_secret_material "verify session (${alias})" "$vs"

  if ! printf '%s' "$vs" | jq -e --arg a "$alias" '
    .kind == "session"
    and .session_ok == true
    and (.whoami.binding_alias // "") == $a
    and (.whoami.seal_ok == true)
    and (.safe_next.ready == true)
    and ((.safe_next.binding // $a) == $a)
  ' >/dev/null 2>&1; then
    local kind session_ok who_alias
    kind="$(printf '%s' "$vs" | jq -r '.kind // empty')"
    session_ok="$(printf '%s' "$vs" | jq -r 'if .session_ok == true then "true" elif .session_ok == false then "false" else empty end')"
    who_alias="$(printf '%s' "$vs" | jq -r '.whoami.binding_alias // empty')"
    safe_leave
    die "verify session gate failed for ${alias} (kind=${kind} session_ok=${session_ok} whoami=${who_alias} exit=${vs_rc})"
  fi
  ok "verify session session_ok=true binding=${alias}"

  log "agent report --json ready/gate (${alias})"
  set +e
  report="$(locus agent report --json 2>/dev/null)"
  report_rc=$?
  set -e
  if ! printf '%s' "$report" | jq -e . >/dev/null 2>&1; then
    safe_leave
    die "agent report did not emit JSON for ${alias} (exit=${report_rc})"
  fi
  assert_no_secret_material "agent report (${alias})" "$report"

  # ready/gate: status=ready, pin matches alias, seal healthy, no unsafe
  if ! printf '%s' "$report" | jq -e --arg a "$alias" '
    .ready == true
    and .status == "ready"
    and .exit_code == 0
    and (.pin != null)
    and .pin.alias == $a
    and .pin.seal_ok == true
    and .pin.expired == false
    and (.status_oneline | startswith($a + ":"))
    and .mcp_command == "locus-mcp"
    and (.required_servers | index("locus") != null)
  ' >/dev/null 2>&1; then
    local ready status oneline pin_alias
    ready="$(printf '%s' "$report" | jq -r 'if .ready == true then "true" elif .ready == false then "false" else empty end')"
    status="$(printf '%s' "$report" | jq -r '.status // empty')"
    oneline="$(printf '%s' "$report" | jq -r '.status_oneline // empty')"
    pin_alias="$(printf '%s' "$report" | jq -r '.pin.alias // empty')"
    safe_leave
    die "agent report ready/gate failed for ${alias} (ready=${ready} status=${status} oneline=${oneline} pin=${pin_alias} exit=${report_rc})"
  fi

  # Print only non-secret gate fields
  printf '%s' "$report" | jq -c --arg a "$alias" '{
    alias: $a,
    ready,
    status,
    status_oneline,
    pin_alias: .pin.alias,
    pin_tenant: .pin.tenant,
    seal_ok: .pin.seal_ok,
    mcp: .mcp_registered
  }' | scrub_secrets
  ok "agent report ready under ${alias}"

  log "leave ${alias}"
  locus leave >/dev/null
  ok "left ${alias}"
}

# ── Main ─────────────────────────────────────────────────────────────────────
log "multi-account dogfood walk"

ensure_tools
echo "locus: $(command -v locus) ($(locus --version 2>/dev/null || true))"
echo "home: ${LOCUS_HOME:-~/.locus}"
echo "personal_alias: ${PERSONAL:-<unset>}"
echo "client_alias: ${CLIENT:-<unset>}"
echo "require_multi: ${REQUIRE}"

if [[ -z "$PERSONAL" || -z "$CLIENT" ]]; then
  if [[ "$REQUIRE" == "1" ]]; then
    die "LOCUS_PERSONAL_ALIAS and LOCUS_CLIENT_ALIAS (or args) required when LOCUS_DOGFOOD_REQUIRE_MULTI=1"
  fi
  skip "aliases missing (set LOCUS_PERSONAL_ALIAS + LOCUS_CLIENT_ALIAS or pass args)"
  printf '\nMULTI-ACCOUNT DOGFOOD: soft-skip (aliases missing)\n'
  exit 0
fi

if [[ "$PERSONAL" == "$CLIENT" ]]; then
  die "personal and client aliases must differ (got '${PERSONAL}')"
fi

MISSING=0
if ! binding_exists "$PERSONAL"; then
  warn "binding not found: ${PERSONAL}"
  MISSING=1
fi
if ! binding_exists "$CLIENT"; then
  warn "binding not found: ${CLIENT}"
  MISSING=1
fi

if [[ "$MISSING" -ne 0 ]]; then
  if [[ "$REQUIRE" == "1" ]]; then
    die "one or more bindings missing and LOCUS_DOGFOOD_REQUIRE_MULTI=1"
  fi
  skip "binding(s) missing on this home — soft-skip"
  printf '\nMULTI-ACCOUNT DOGFOOD: soft-skip (bindings missing)\n'
  exit 0
fi

# Clear any ambient pin before the walk.
safe_leave

log "walk personal (${PERSONAL})"
gate_alias "$PERSONAL"

log "walk client (${CLIENT})"
gate_alias "$CLIENT"

# Confirm residual identity cleared.
set +e
oneline="$(locus status --oneline 2>/dev/null || true)"
set -e
if [[ -n "$oneline" && "$oneline" != "unpinned" ]]; then
  warn "status after walk is '${oneline}' (expected unpinned); forcing leave"
  safe_leave
fi

log "summary"
printf '  · personal: %s → ready + leave\n' "$PERSONAL"
printf '  · client:   %s → ready + leave\n' "$CLIENT"
printf '  · require:  %s\n' "$REQUIRE"
printf '\nMULTI-ACCOUNT DOGFOOD: ok\n'
exit 0
