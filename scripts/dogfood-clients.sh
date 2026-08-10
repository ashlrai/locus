#!/usr/bin/env bash
# dogfood-clients.sh — probe real AI client installs for multi-client dogfood.
#
# Detects Claude Code / Cursor / Continue config (or install) paths on common
# macOS + Linux locations. For each found *supported* client (claude, cursor),
# runs `locus agent setup --client X --dry-run` (never mutates host configs).
# Optionally runs `locus agent doctor` once when any client is present.
#
# Soft-skip (exit 0 + summary) when clients are missing or Continue is
# detected-but-unsupported. Hard-fail only when:
#   LOCUS_DOGFOOD_REQUIRE_CLIENTS=1 and no supported client found, OR
#   setup fails for a found supported client
#
# Never prints secret values or credential locators. Never --apply.
#
# Usage:
#   scripts/dogfood-clients.sh
#   LOCUS_DOGFOOD_REQUIRE_CLIENTS=1 scripts/dogfood-clients.sh
#   DOGFOOD_CLIENTS=1 scripts/dogfood.sh   # optional soft step from dogfood
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
export PATH="${ROOT}/target/debug:${ROOT}/target/release:${HOME}/.cargo/bin:${PATH}"

REQUIRE="${LOCUS_DOGFOOD_REQUIRE_CLIENTS:-0}"
RUN_DOCTOR="${DOGFOOD_CLIENTS_DOCTOR:-1}"
HOME_DIR="${HOME:-}"
# Optional override for tests / portable probes (default: real $HOME).
# When set, only paths under PROBE_HOME are considered (no /Applications, PATH, or cwd).
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

# Paths may exist; we never cat their contents (could hold tokens in mcp env).
path_exists() { [[ -e "$1" ]]; }

# ── Detection ────────────────────────────────────────────────────────────────
# Return 0 if any marker path exists. Prints first matching path on stdout.
# Markers are install/config roots only — never read file bodies.

detect_claude() {
  local candidates=(
    "${PROBE_HOME}/.claude"
    "${PROBE_HOME}/.claude.json"
    "${PROBE_HOME}/Library/Application Support/Claude"
    "${PROBE_HOME}/.config/claude"
    "${PROBE_HOME}/.config/Claude"
  )
  if [[ "$PROBE_ISOLATED" != "1" ]]; then
    # Project-local Claude Code MCP config (cwd)
    candidates+=("$(pwd)/.mcp.json")
    # App bundles / PATH (macOS + common bins)
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

detect_continue() {
  # Continue.dev — detect only; locus agent setup has no --client continue yet.
  local candidates=(
    "${PROBE_HOME}/.continue/config.json"
    "${PROBE_HOME}/.continue"
    "${PROBE_HOME}/.continue/config.yaml"
    "${PROBE_HOME}/.continue/config.yml"
    "${PROBE_HOME}/.config/continue"
    "${PROBE_HOME}/Library/Application Support/Code/User/globalStorage/continue.continue"
    "${PROBE_HOME}/.config/Code/User/globalStorage/continue.continue"
    "${PROBE_HOME}/.vscode/extensions"
  )
  local p
  for p in "${candidates[@]}"; do
    # For the extensions dir, only count if a continue package is present.
    if [[ "$p" == *"/extensions" ]]; then
      if compgen -G "${p}/continue.continue-*" >/dev/null 2>&1 \
        || compgen -G "${p}/Continue.continue-*" >/dev/null 2>&1; then
        printf '%s\n' "$p"
        return 0
      fi
      continue
    fi
    if path_exists "$p"; then
      printf '%s\n' "$p"
      return 0
    fi
  done
  return 1
}

# Redact anything that looks like a secret if it ever leaks into setup output.
# (setup --dry-run should only print paths + planned actions.)
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
  # Never echo raw secrets; scrub defensively and drop credential_ref-like lines.
  printf '%s\n' "$out" | scrub_secrets | grep -Eiv \
    'credential_ref|secret_value|password|api[_-]?key|token[[:space:]]*=' \
    || true
  return "$rc"
}

# ── Main ─────────────────────────────────────────────────────────────────────
log "multi-client install probe"

ensure_locus
echo "locus: $(command -v locus) ($(locus --version 2>/dev/null || true))"
echo "probe home: ${PROBE_HOME}"
echo "cwd: $(pwd)"

FOUND_SUPPORTED=0
FOUND_ANY=0
FAILED=0
SUMMARY=()

# Claude Code
CLAUDE_PATH=""
if CLAUDE_PATH="$(detect_claude)"; then
  FOUND_ANY=1
  FOUND_SUPPORTED=$((FOUND_SUPPORTED + 1))
  ok "claude detected @ ${CLAUDE_PATH}"
  log "locus agent setup --dry-run --client claude"
  if run_setup_dry claude; then
    ok "agent setup dry-run claude"
    SUMMARY+=("claude:found+setup-ok (${CLAUDE_PATH})")
  else
    warn "agent setup dry-run failed for claude"
    FAILED=1
    SUMMARY+=("claude:found+setup-FAIL (${CLAUDE_PATH})")
  fi
else
  skip "claude (no config/install markers)"
  SUMMARY+=("claude:missing")
fi

# Cursor
CURSOR_PATH=""
if CURSOR_PATH="$(detect_cursor)"; then
  FOUND_ANY=1
  FOUND_SUPPORTED=$((FOUND_SUPPORTED + 1))
  ok "cursor detected @ ${CURSOR_PATH}"
  log "locus agent setup --dry-run --client cursor"
  if run_setup_dry cursor; then
    ok "agent setup dry-run cursor"
    SUMMARY+=("cursor:found+setup-ok (${CURSOR_PATH})")
  else
    warn "agent setup dry-run failed for cursor"
    FAILED=1
    SUMMARY+=("cursor:found+setup-FAIL (${CURSOR_PATH})")
  fi
else
  skip "cursor (no config/install markers)"
  SUMMARY+=("cursor:missing")
fi

# Continue (detect only — no setup client yet)
CONTINUE_PATH=""
if CONTINUE_PATH="$(detect_continue)"; then
  FOUND_ANY=1
  ok "continue detected @ ${CONTINUE_PATH}"
  skip "continue setup (no --client continue; wire MCP manually if needed)"
  SUMMARY+=("continue:found-unsupported (${CONTINUE_PATH})")
else
  skip "continue (no config/install markers)"
  SUMMARY+=("continue:missing")
fi

# Doctor once when any supported client is present (optional)
if [[ "$FOUND_SUPPORTED" -ge 1 && "$RUN_DOCTOR" == "1" ]]; then
  log "locus agent doctor (optional multi-client context)"
  set +e
  doctor_out="$(locus agent doctor 2>&1)"
  doctor_rc=$?
  set -e
  printf '%s\n' "$doctor_out" | scrub_secrets | grep -Eiv \
    'credential_ref|secret_value|password|api[_-]?key|token[[:space:]]*=' \
    || true
  if [[ "$doctor_rc" -eq 0 ]]; then
    ok "agent doctor (exit=0)"
  else
    # Soft: doctor may WARN on real homes without a pin; do not hard-fail here.
    warn "agent doctor exit=${doctor_rc} (non-fatal for client probe)"
  fi
fi

log "summary"
for line in "${SUMMARY[@]}"; do
  printf '  · %s\n' "$line"
done
printf '  supported_found=%s any_found=%s setup_failures=%s require=%s\n' \
  "$FOUND_SUPPORTED" "$FOUND_ANY" "$FAILED" "$REQUIRE"

if [[ "$FAILED" -ne 0 ]]; then
  die "setup dry-run failed for one or more found clients"
fi

if [[ "$FOUND_SUPPORTED" -eq 0 ]]; then
  if [[ "$REQUIRE" == "1" ]]; then
    die "no supported clients found (claude/cursor) and LOCUS_DOGFOOD_REQUIRE_CLIENTS=1"
  fi
  printf '\nCLIENT PROBE: none found (soft-skip; set LOCUS_DOGFOOD_REQUIRE_CLIENTS=1 to hard-fail)\n'
  exit 0
fi

printf '\nCLIENT PROBE: ok (%s supported client(s))\n' "$FOUND_SUPPORTED"
exit 0
