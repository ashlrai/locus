#!/usr/bin/env bash
# Stubbed probe: the e2e suite's initial temp allocation beneath a
# space-containing TMPDIR. The script's own section-2 block runs verbatim
# with stub log/ok/die and a recording mktemp — the template must reach
# mktemp as ONE argument and the created home must land directly beneath
# the supplied root.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SRC="$ROOT/scripts/e2e.sh"
TEST_DIR="$(mktemp -d "${TMPDIR:-/tmp}/locus-e2e-spaced-test.XXXXXX")"
trap 'rm -rf "$TEST_DIR"' EXIT

SPACED="$TEST_DIR/tmp root with spaces"
mkdir -p "$SPACED"
touch "$SPACED/unrelated"

# Extract the suite's own selection block verbatim: create_locus_home()
# through the closing fi of the TMPDIR containment check. Bounds are
# asserted so a drifted script fails here loudly rather than probing the
# wrong slice.
begin="$(grep -n '^create_locus_home() {' "$SRC" | head -1 | cut -d: -f1)"
[[ -n "$begin" ]] || { echo 'FAIL: create_locus_home not found in e2e.sh' >&2; exit 1; }
ok_line="$(grep -n 'ok "isolated home is created directly beneath TMPDIR"' "$SRC" | head -1 | cut -d: -f1)"
[[ -n "$ok_line" && "$ok_line" -gt "$begin" ]] \
  || { echo 'FAIL: TMPDIR containment check not found in e2e.sh' >&2; exit 1; }
[[ "$(sed -n "$((ok_line + 1))p" "$SRC")" == "fi" ]] \
  || { echo 'FAIL: extraction bounds drifted — expected fi after the containment ok' >&2; exit 1; }
BLOCK="$TEST_DIR/block.sh"
sed -n "${begin},$((ok_line + 1))p" "$SRC" >"$BLOCK"

# Stubs: quiet log/ok/die plus a mktemp that records its argument count
# and template, then allocates for real.
export MKTEMP_ARGC="$TEST_DIR/mktemp-argc"
export MKTEMP_TEMPLATE="$TEST_DIR/mktemp-template"
export MKTEMP_CREATED="$TEST_DIR/mktemp-created"
export OBSERVED_HOME="$TEST_DIR/observed-home"
export OK_LOG="$TEST_DIR/ok-log"
cat >"$TEST_DIR/prelude.sh" <<'EOF'
set -euo pipefail
log() { :; }
ok() { printf 'ok %s\n' "$*" >>"$OK_LOG"; }
die() { printf 'FAIL %s\n' "$*" >&2; exit 1; }
mktemp() {
  printf '%s' "$#" >"$MKTEMP_ARGC"
  printf '%s' "${2:-}" >"$MKTEMP_TEMPLATE"
  local created
  created="$(command mktemp "$@")" || return
  printf '%s' "$created" >"$MKTEMP_CREATED"
  printf '%s\n' "$created"
}
EOF

status=0
env TMPDIR="$SPACED" PRELUDE="$TEST_DIR/prelude.sh" BLOCK="$BLOCK" \
  bash -c 'source "$PRELUDE"; source "$BLOCK"; printf %s "$LOCUS_HOME" >"$OBSERVED_HOME"' \
  || status=$?

[[ "$status" -eq 0 ]] \
  || { echo "FAIL: selection block exited $status under a spaced TMPDIR" >&2; exit 1; }
[[ "$(cat "$MKTEMP_ARGC")" == "2" ]] \
  || { echo 'FAIL: mktemp did not receive exactly -d and one template argument' >&2; exit 1; }
[[ "$(cat "$MKTEMP_TEMPLATE")" == "$SPACED/locus-e2e.XXXXXX" ]] \
  || { echo 'FAIL: spaced root did not reach mktemp inside a single template argument' >&2; exit 1; }
home="$(cat "$OBSERVED_HOME")"
[[ "$home" == "$(cat "$MKTEMP_CREATED")" ]] \
  || { echo 'FAIL: assigned home differs from the path mktemp created' >&2; exit 1; }
[[ "${home%/*}" == "$SPACED" && "${home##*/}" == locus-e2e.* ]] \
  || { echo 'FAIL: home was not created directly beneath the supplied TMPDIR' >&2; exit 1; }
[[ "$home" == *" "* ]] || { echo 'FAIL: home path lost its spaces' >&2; exit 1; }
[[ ! -e "$home" && ! -L "$home" ]] || { echo 'FAIL: home survived exit cleanup' >&2; exit 1; }
[[ -f "$SPACED/unrelated" ]] \
  || { echo 'FAIL: cleanup touched unrelated data in the spaced root' >&2; exit 1; }
[[ "$(find "$SPACED" -mindepth 1 -maxdepth 1 | wc -l | tr -d ' ')" == "1" ]] \
  || { echo 'FAIL: stray entries left beneath the spaced root' >&2; exit 1; }
grep -Fxq 'ok isolated home is created directly beneath TMPDIR' "$OK_LOG" \
  || { echo 'FAIL: containment check did not confirm the home' >&2; exit 1; }

echo 'PASS: initial temp path stays one argument and lands beneath the supplied spaced TMPDIR'
