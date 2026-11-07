#!/usr/bin/env bash
# Proves scripts/e2e.sh's drop-in mechanism: a temp step file runs (in sorted
# order, non-.sh files ignored) and a failing one fails the run. Needs no
# built binaries.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/locus-dropin.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT

fail() { printf 'FAIL %s\n' "$*" >&2; exit 1; }

# Wiring: e2e.sh sources the runner and calls it after its own steps, before
# the PASS summary.
src_line="$(grep -n 'source "\$ROOT/scripts/e2e-dropin.sh"' "$ROOT/scripts/e2e.sh" | cut -d: -f1)"
run_line="$(grep -n '^e2e_run_dropins$' "$ROOT/scripts/e2e.sh" | cut -d: -f1)"
pass_line="$(grep -n "e2e PASS" "$ROOT/scripts/e2e.sh" | cut -d: -f1)"
[[ -n "$src_line" && -n "$run_line" && -n "$pass_line" ]] || fail "e2e.sh does not wire the drop-in runner"
(( src_line < run_line && run_line < pass_line )) || fail "drop-ins must run before the PASS summary"
[[ -f "$ROOT/scripts/e2e.d/README.md" ]] || fail "missing scripts/e2e.d/README.md"

# Run the runner in a subshell with the same helper contract e2e.sh provides.
run_dropins() { # dir -> combined output on stdout, status returned
  # A fresh Bash process keeps outer if/|| contexts from disabling errexit.
  "$BASH" -euo pipefail -s -- "$ROOT" "$1" <<'DROPIN_CHILD' 2>&1
ROOT="$1"
export ROOT
log()  { printf '==> %s\n' "$*"; }
ok()   { printf 'ok %s\n' "$*"; }
skip() { printf 'skip %s\n' "$*"; }
die()  { printf 'FAIL %s\n' "$*" >&2; exit 1; }
# shellcheck source=scripts/e2e-dropin.sh
source "$ROOT/scripts/e2e-dropin.sh"
e2e_run_dropins "$2"
echo DROPINS_DONE
DROPIN_CHILD
}

# 1. passing steps run, in sorted order; non-.sh files are ignored.
d1="$tmp/pass"; mkdir "$d1"
echo 'ok "step-b"' >"$d1/20-b.sh"
echo 'ok "step-a"' >"$d1/10-a.sh"
echo 'ok "step-ignored"' >"$d1/30-notes.txt"
out="$(run_dropins "$d1")" || fail "passing drop-ins failed: $out"
grep -q 'ok step-a' <<<"$out" && grep -q 'ok step-b' <<<"$out" || fail "temp step did not run: $out"
grep -q 'step-ignored' <<<"$out" && fail "non-.sh file ran"
a_pos="$(grep -n 'ok step-a' <<<"$out" | cut -d: -f1)"
b_pos="$(grep -n 'ok step-b' <<<"$out" | cut -d: -f1)"
(( a_pos < b_pos )) || fail "steps ran out of sorted order"
grep -q DROPINS_DONE <<<"$out" || fail "run did not complete"

# 2. a failing step (die) fails the run and stops later steps.
d2="$tmp/die"; mkdir "$d2"
echo 'die "boom"' >"$d2/10-bad.sh"
echo 'ok "after-bad"' >"$d2/20-after.sh"
if out="$(run_dropins "$d2")"; then fail "die step did not fail the run"; fi
grep -q 'after-bad' <<<"$out" && fail "step after a failure still ran"

# 3. a bare failing command (set -e) and an explicit exit also fail the run.
d3="$tmp/false"; mkdir "$d3"; echo 'false' >"$d3/10-false.sh"
if run_dropins "$d3" >/dev/null; then fail "failing command did not fail the run"; fi
d4="$tmp/exit"; mkdir "$d4"; echo 'exit 3' >"$d4/10-exit.sh"
if run_dropins "$d4" >/dev/null; then fail "exit step did not fail the run"; fi

# 4. empty or missing directory is a no-op.
d5="$tmp/empty"; mkdir "$d5"
run_dropins "$d5" | grep -q DROPINS_DONE || fail "empty dir should pass"
run_dropins "$tmp/missing" | grep -q DROPINS_DONE || fail "missing dir should pass"

printf 'e2e drop-in check PASS\n'
