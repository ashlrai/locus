#!/usr/bin/env bash
# dogfood-dual-ide.sh — dual-IDE dogfood matrix without requiring secrets.
#
# For each found client (claude, cursor):
#   1. locus agent setup --dry-run --client X (never --apply)
#   2. Resolve MCP config JSON path(s) and check they contain "locus"
#      (key/string presence only — never print config bodies or secret values)
#
# When personal + client aliases are set (env or args):
#   run scripts/dogfood-multi-account.sh + client probe (dogfood-clients.sh)
#
# Prints a matrix:
#   client | found | setup_dry | locus_reg | multi_account
#
# Soft-skip (exit 0 + matrix) when clients/aliases are missing.
# Hard-fail only when:
#   LOCUS_DOGFOOD_REQUIRE_DUAL=1 and no supported client found, OR
#   setup dry-run fails for a found client, OR
#   multi-account walk fails when aliases were provided, OR
#   LOCUS_DOGFOOD_REQUIRE_DUAL=1 and no client has locus registered
#
# Never prints secret values, CredentialRef locators, or full MCP env maps.
# Never --apply.
#
# Usage:
#   scripts/dogfood-dual-ide.sh
#   LOCUS_PERSONAL_ALIAS=personal LOCUS_CLIENT_ALIAS=client-a scripts/dogfood-dual-ide.sh
#   scripts/dogfood-dual-ide.sh personal client-a
#   LOCUS_DOGFOOD_REQUIRE_DUAL=1 scripts/dogfood-dual-ide.sh personal client-a
#   LOCUS_DOGFOOD_PROBE_HOME=/tmp/fake-home scripts/dogfood-dual-ide.sh  # isolated detect
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
export PATH="${ROOT}/target/debug:${ROOT}/target/release:${HOME}/.cargo/bin:${PATH}"

PERSONAL="${1:-${LOCUS_PERSONAL_ALIAS:-}}"
CLIENT="${2:-${LOCUS_CLIENT_ALIAS:-}}"
REQUIRE="${LOCUS_DOGFOOD_REQUIRE_DUAL:-0}"
HOME_DIR="${HOME:-}"
# Optional override for tests / portable probes (default: real $HOME).
PROBE_HOME="${LOCUS_DOGFOOD_PROBE_HOME:-$HOME_DIR}"
PROBE_ISOLATED=0
if [[ -n "${LOCUS_DOGFOOD_PROBE_HOME:-}" ]]; then
  PROBE_ISOLATED=1
fi

log()  { printf '\n==> %s\n' "$*"; }
ok()   { printf '  ok  %s\n' "$*"; }
skip() { printf '  skip %s\n' "$*"; }
warn() { printf '  warn %s\n' "$*" >&2; }
die()  { printf '  FAIL %s\n' "$*" >&2; exit 1; }

path_exists() { [[ -e "$1" ]]; }

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

# ── Client install detection (paths only — never read bodies) ───────────────

detect_claude() {
  local candidates=(
    "${PROBE_HOME}/.claude"
    "${PROBE_HOME}/.claude.json"
    "${PROBE_HOME}/Library/Application Support/Claude"
    "${PROBE_HOME}/.config/claude"
    "${PROBE_HOME}/.config/Claude"
  )
  if [[ "$PROBE_ISOLATED" != "1" ]]; then
    candidates+=("$(pwd)/.mcp.json")
    if [[ "$(uname -s)" == "Darwin" ]]; then
      candidates+=("/Applications/Claude.app")
    fi
  fi
  local p
  for p in "${candidates[@]}"; do
    if path_exists "$p"; then
      printf '%s\n' "$p"
      return 0
    fi
  done
  if [[ "$PROBE_ISOLATED" != "1" ]] && command -v claude >/dev/null 2>&1; then
    command -v claude
    return 0
  fi
  return 1
}

detect_cursor() {
  local candidates=(
    "${PROBE_HOME}/.cursor/mcp.json"
    "${PROBE_HOME}/.cursor"
    "${PROBE_HOME}/Library/Application Support/Cursor"
    "${PROBE_HOME}/.config/Cursor"
    "${PROBE_HOME}/.config/cursor"
  )
  if [[ "$PROBE_ISOLATED" != "1" ]]; then
    candidates+=("$(pwd)/.cursor/mcp.json")
    candidates+=("$(pwd)/.cursor")
    if [[ "$(uname -s)" == "Darwin" ]]; then
      candidates+=("/Applications/Cursor.app")
    fi
  fi
  local p
  for p in "${candidates[@]}"; do
    if path_exists "$p"; then
      printf '%s\n' "$p"
      return 0
    fi
  done
  if [[ "$PROBE_ISOLATED" != "1" ]] && command -v cursor >/dev/null 2>&1; then
    command -v cursor
    return 0
  fi
  return 1
}

# MCP config candidates where locus agent setup / probe look for registration.
# Always include cwd (agent setup writes project-local configs there).
# Also probe PROBE_HOME global paths. Prints existing paths only — never bodies.
mcp_config_paths_claude() {
  local paths=(
    "$(pwd)/.mcp.json"
    "${PROBE_HOME}/.mcp.json"
  )
  local p
  for p in "${paths[@]}"; do
    if [[ -f "$p" ]]; then
      printf '%s\n' "$p"
    fi
  done
}

mcp_config_paths_cursor() {
  local paths=(
    "$(pwd)/.cursor/mcp.json"
    "${PROBE_HOME}/.cursor/mcp.json"
  )
  local p
  for p in "${paths[@]}"; do
    if [[ -f "$p" ]]; then
      printf '%s\n' "$p"
    fi
  done
}

# True if MCP JSON has a mcpServers.locus entry (or literal "locus" key string).
# Never prints file contents or env values — exit status + path only.
mcp_json_has_locus() {
  local path="$1"
  [[ -f "$path" ]] || return 1
  if command -v jq >/dev/null 2>&1; then
    # Key presence only; discard any value payload.
    if jq -e '
      (.mcpServers // {}) | has("locus")
      or ((.mcpServers // {}) | keys | map(ascii_downcase) | index("locus") != null)
    ' "$path" >/dev/null 2>&1; then
      return 0
    fi
    return 1
  fi
  # Fallback: string match for "locus" server name without dumping the file.
  # Prefer quoted key form used by Claude/Cursor JSON.
  if grep -Eq '"locus"[[:space:]]*:' "$path" 2>/dev/null; then
    return 0
  fi
  if grep -Eq '"mcpServers"[[:space:]]*:.*"locus"' "$path" 2>/dev/null; then
    return 0
  fi
  return 1
}

# Resolve first MCP path that contains locus; else first existing path; else empty.
# Sets globals: MCP_PATH_OUT, LOCUS_REG_OUT (yes|no|n/a)
resolve_locus_registration() {
  local client="$1"
  local path
  MCP_PATH_OUT=""
  LOCUS_REG_OUT="n/a"

  local paths=()
  case "$client" in
    claude)
      while IFS= read -r path; do
        [[ -n "$path" ]] && paths+=("$path")
      done < <(mcp_config_paths_claude)
      ;;
    cursor)
      while IFS= read -r path; do
        [[ -n "$path" ]] && paths+=("$path")
      done < <(mcp_config_paths_cursor)
      ;;
    *)
      return 1
      ;;
  esac

  if [[ "${#paths[@]}" -eq 0 ]]; then
    LOCUS_REG_OUT="no"
    MCP_PATH_OUT="(no mcp json found)"
    return 0
  fi

  for path in "${paths[@]}"; do
    if mcp_json_has_locus "$path"; then
      MCP_PATH_OUT="$path"
      LOCUS_REG_OUT="yes"
      return 0
    fi
  done

  # Config exists but no locus key — report first path for operators.
  MCP_PATH_OUT="${paths[0]}"
  LOCUS_REG_OUT="no"
  return 0
}

ensure_locus() {
  if command -v locus >/dev/null 2>&1 && locus agent setup --help >/dev/null 2>&1; then
    return 0
  fi
  command -v cargo >/dev/null 2>&1 || die "locus (with agent setup) not on PATH and cargo missing"
  log "building locus-cli for agent setup"
  (cd "$ROOT" && cargo build -q -p locus-cli)
  export PATH="${ROOT}/target/debug:${PATH}"
  command -v locus >/dev/null 2>&1 || die "locus binary missing after build"
}

run_setup_dry() {
  local client="$1"
  local out rc
  set +e
  out="$(locus agent setup --dry-run --client "$client" 2>&1)"
  rc=$?
  set -e
  # Never echo raw secrets; scrub and drop credential-looking lines.
  printf '%s\n' "$out" | scrub_secrets | grep -Eiv \
    'credential_ref|secret_value|password|api[_-]?key|token[[:space:]]*=' \
    || true
  return "$rc"
}

# Optional: non-secret mcp_registered flags from agent report (cwd + home probe).
probe_report_mcp() {
  local report
  set +e
  report="$(locus agent report --json 2>/dev/null)"
  set -e
  if ! command -v jq >/dev/null 2>&1; then
    return 0
  fi
  if ! printf '%s' "$report" | jq -e . >/dev/null 2>&1; then
    return 0
  fi
  # Print only boolean flags — never pin secrets (report should not have them).
  printf '%s' "$report" | jq -c '{
    mcp_registered,
    status,
    ready
  }' 2>/dev/null | scrub_secrets || true
}

pad() {
  # left-pad/truncate field for table
  local s="$1" w="$2"
  printf '%-*s' "$w" "${s:0:$w}"
}

# ── Main ─────────────────────────────────────────────────────────────────────
log "dual-IDE dogfood matrix (no secrets)"

ensure_locus
echo "locus: $(command -v locus) ($(locus --version 2>/dev/null || true))"
echo "probe home: ${PROBE_HOME}"
echo "cwd: $(pwd)"
echo "personal_alias: ${PERSONAL:-<unset>}"
echo "client_alias: ${CLIENT:-<unset>}"
echo "require_dual: ${REQUIRE}"

FOUND_SUPPORTED=0
SETUP_FAILED=0
LOCUS_ANY=0
MULTI_STATUS="skipped"
MULTI_DETAIL="aliases unset"
FAILED=0

# Matrix rows: client|found|setup|locus_reg|mcp_path|multi
declare -a ROW_CLIENT ROW_FOUND ROW_SETUP ROW_LOCUS ROW_PATH

probe_one_client() {
  local name="$1"
  local detect_fn="$2"
  local found_path="" setup_status="n/a" locus_status="n/a" mcp_path="—"

  if found_path="$($detect_fn)"; then
    FOUND_SUPPORTED=$((FOUND_SUPPORTED + 1))
    ok "${name} detected @ ${found_path}"

    log "locus agent setup --dry-run --client ${name}"
    if run_setup_dry "$name"; then
      ok "agent setup dry-run ${name}"
      setup_status="ok"
    else
      warn "agent setup dry-run failed for ${name}"
      setup_status="FAIL"
      SETUP_FAILED=1
      FAILED=1
    fi

    resolve_locus_registration "$name"
    locus_status="$LOCUS_REG_OUT"
    mcp_path="$MCP_PATH_OUT"
    if [[ "$locus_status" == "yes" ]]; then
      LOCUS_ANY=1
      ok "locus registered in MCP config @ ${mcp_path}"
    else
      warn "locus not registered in MCP config for ${name} (${mcp_path})"
    fi

    ROW_CLIENT+=("$name")
    ROW_FOUND+=("yes")
    ROW_SETUP+=("$setup_status")
    ROW_LOCUS+=("$locus_status")
    ROW_PATH+=("$mcp_path")
  else
    skip "${name} (no config/install markers)"
    ROW_CLIENT+=("$name")
    ROW_FOUND+=("no")
    ROW_SETUP+=("n/a")
    ROW_LOCUS+=("n/a")
    ROW_PATH+=("—")
  fi
}

probe_one_client claude detect_claude
probe_one_client cursor detect_cursor

# Agent report mcp flags (non-secret) when any client found
if [[ "$FOUND_SUPPORTED" -ge 1 ]]; then
  log "agent report mcp_registered (flags only)"
  probe_report_mcp || true
fi

# Multi-account walk when aliases provided
if [[ -n "$PERSONAL" && -n "$CLIENT" ]]; then
  if [[ ! -f "$ROOT/scripts/dogfood-multi-account.sh" ]]; then
    die "dogfood-multi-account.sh missing at $ROOT/scripts/dogfood-multi-account.sh"
  fi
  log "multi-account walk (${PERSONAL} → ${CLIENT}) + client probe"
  set +e
  # Propagate require flag if dual require is on; multi has its own soft-skip.
  if [[ "$REQUIRE" == "1" ]]; then
    LOCUS_DOGFOOD_REQUIRE_MULTI=1 \
      bash "$ROOT/scripts/dogfood-multi-account.sh" "$PERSONAL" "$CLIENT"
  else
    bash "$ROOT/scripts/dogfood-multi-account.sh" "$PERSONAL" "$CLIENT"
  fi
  multi_rc=$?
  set -e
  if [[ "$multi_rc" -eq 0 ]]; then
    if [[ "$REQUIRE" == "1" ]]; then
      MULTI_STATUS="walked"
      MULTI_DETAIL="${PERSONAL}→${CLIENT}"
      ok "multi-account walk ok"
    else
      # Without require, multi may soft-skip missing bindings. Refine via list.
      if command -v locus >/dev/null 2>&1 && command -v jq >/dev/null 2>&1; then
        list="$(locus binding list --json 2>/dev/null || echo '[]')"
        if printf '%s' "$list" | jq -e --arg p "$PERSONAL" --arg c "$CLIENT" '
          (type == "array")
          and (map(.alias) | index($p) != null)
          and (map(.alias) | index($c) != null)
        ' >/dev/null 2>&1; then
          MULTI_STATUS="walked"
          MULTI_DETAIL="${PERSONAL}→${CLIENT}"
          ok "multi-account walk ok"
        else
          MULTI_STATUS="skipped"
          MULTI_DETAIL="bindings missing or soft-skip"
          skip "multi-account soft-skip (bindings missing?)"
        fi
      else
        MULTI_STATUS="walked"
        MULTI_DETAIL="${PERSONAL}→${CLIENT} (exit 0)"
        ok "multi-account script exit 0"
      fi
    fi
  else
    MULTI_STATUS="FAIL"
    MULTI_DETAIL="exit=${multi_rc}"
    FAILED=1
    warn "multi-account walk failed (exit=${multi_rc})"
  fi
else
  skip "multi-account (set LOCUS_PERSONAL_ALIAS + LOCUS_CLIENT_ALIAS or pass args)"
  MULTI_STATUS="skipped"
  MULTI_DETAIL="aliases unset"
fi

# When multi-account aliases were provided, also run the install probe for a
# combined dual-IDE + multi-account operator path (doctor off; setup already dry-ran).
if [[ -n "$PERSONAL" && -n "$CLIENT" && -f "$ROOT/scripts/dogfood-clients.sh" ]]; then
  log "client install probe (dogfood-clients.sh)"
  set +e
  DOGFOOD_CLIENTS_DOCTOR=0 bash "$ROOT/scripts/dogfood-clients.sh"
  clients_rc=$?
  set -e
  if [[ "$clients_rc" -ne 0 ]]; then
    warn "dogfood-clients.sh exit=${clients_rc}"
    FAILED=1
  else
    ok "dogfood-clients.sh"
  fi
fi

# ── Matrix ───────────────────────────────────────────────────────────────────
log "matrix"
printf '\n'
printf '| %s | %s | %s | %s | %s |\n' \
  "$(pad client 8)" \
  "$(pad found 5)" \
  "$(pad setup_dry 10)" \
  "$(pad locus_reg 9)" \
  "$(pad multi_account 22)"
printf '| %s | %s | %s | %s | %s |\n' \
  "$(pad '--------' 8)" \
  "$(pad '-----' 5)" \
  "$(pad '----------' 10)" \
  "$(pad '---------' 9)" \
  "$(pad '----------------------' 22)"

i=0
while [[ "$i" -lt "${#ROW_CLIENT[@]}" ]]; do
  printf '| %s | %s | %s | %s | %s |\n' \
    "$(pad "${ROW_CLIENT[$i]}" 8)" \
    "$(pad "${ROW_FOUND[$i]}" 5)" \
    "$(pad "${ROW_SETUP[$i]}" 10)" \
    "$(pad "${ROW_LOCUS[$i]}" 9)" \
    "$(pad "${MULTI_STATUS}" 22)"
  i=$((i + 1))
done

printf '\n'
printf '  mcp paths (non-secret):\n'
i=0
while [[ "$i" -lt "${#ROW_CLIENT[@]}" ]]; do
  printf '    · %s: found=%s locus=%s path=%s\n' \
    "${ROW_CLIENT[$i]}" "${ROW_FOUND[$i]}" "${ROW_LOCUS[$i]}" "${ROW_PATH[$i]}"
  i=$((i + 1))
done
printf '  multi-account: %s (%s)\n' "$MULTI_STATUS" "$MULTI_DETAIL"
printf '  supported_found=%s locus_any=%s setup_failures=%s require=%s\n' \
  "$FOUND_SUPPORTED" "$LOCUS_ANY" "$SETUP_FAILED" "$REQUIRE"

# ── Exit policy ──────────────────────────────────────────────────────────────
if [[ "$SETUP_FAILED" -ne 0 ]]; then
  die "setup dry-run failed for one or more found clients"
fi

if [[ "$MULTI_STATUS" == "FAIL" ]]; then
  die "multi-account walk failed"
fi

if [[ "$FOUND_SUPPORTED" -eq 0 ]]; then
  if [[ "$REQUIRE" == "1" ]]; then
    die "no supported clients found (claude/cursor) and LOCUS_DOGFOOD_REQUIRE_DUAL=1"
  fi
  printf '\nDUAL-IDE DOGFOOD: none found (soft-skip; set LOCUS_DOGFOOD_REQUIRE_DUAL=1 to hard-fail)\n'
  exit 0
fi

if [[ "$REQUIRE" == "1" && "$LOCUS_ANY" -eq 0 ]]; then
  die "no client has locus registered in MCP config and LOCUS_DOGFOOD_REQUIRE_DUAL=1 (run: locus agent setup --apply --client claude|cursor)"
fi

if [[ "$FAILED" -ne 0 ]]; then
  die "dual-IDE dogfood had failures (see matrix)"
fi

printf '\nDUAL-IDE DOGFOOD: ok (matrix above; secrets never printed)\n'
exit 0
