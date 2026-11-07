#!/usr/bin/env bash
# Run: bash scripts/tests/dogfood-tmpdir.sh
# Exercise the real dogfood home selection beneath an explicit TMPDIR: the
# allocated home must be created directly inside the supplied root, reported
# as the isolated LOCUS_HOME, and removed on exit without disturbing
# unrelated content in that root.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
REAL_MKTEMP="$(command -v mktemp)"
TEST_ROOT="$("$REAL_MKTEMP" -d "${TMPDIR:-/tmp}/locus-dogfood-tmpdir-test.XXXXXX")"
# An inherited trailing-slash TMPDIR leaves a doubled separator in the mktemp
# path; collapse it so every comparison below works on one canonical spelling.
TEST_ROOT="$(cd "$TEST_ROOT" && pwd)"
trap 'rm -rf "$TEST_ROOT"' EXIT

fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }

SCRATCH="$TEST_ROOT/controlled tmp root"
mkdir -p "$SCRATCH"
printf 'keep' > "$SCRATCH/sentinel"

# dogfood.sh resolves its repo root from its own path and prepends
# $ROOT/target/debug to PATH, so a fixture checkout — the copied script plus
# stub binaries — runs the real mainline with every external command stubbed.
# The recording mktemp proves the template the script selects; the locus stub
# asserts containment while the home exists, then stops the run at a
# controlled quickstart failure so no probe ever runs.
FIXTURE="$TEST_ROOT/fixture"
mkdir -p "$FIXTURE/scripts" "$FIXTURE/target/debug"
cp "$ROOT/scripts/dogfood.sh" "$FIXTURE/scripts/dogfood.sh"

cat > "$FIXTURE/target/debug/cargo" <<'EOF'
#!/bin/sh
exit 0
EOF
cat > "$FIXTURE/target/debug/locus-mcp" <<'EOF'
#!/bin/sh
exit 0
EOF
cat > "$FIXTURE/target/debug/mktemp" <<EOF
#!/usr/bin/env bash
printf '%s\n' "\${!#}" >> "\$MKTEMP_TEMPLATES"
exec "$REAL_MKTEMP" "\$@"
EOF
cat > "$FIXTURE/target/debug/locus" <<'EOF'
#!/usr/bin/env bash
case "$1" in
  init)
    # Containment must be physical: the allocated home is a real directory
    # entry inside the controlled root, not a link, and the run already cds
    # into the project beneath it.
    [[ -d "$LOCUS_HOME" && ! -L "$LOCUS_HOME" ]] || exit 71
    [[ -d "$TMPDIR/${LOCUS_HOME##*/}" ]] || exit 72
    [[ "$PWD" == "$LOCUS_HOME/project" ]] || exit 73
    [[ "$(find "$TMPDIR" -mindepth 1 -maxdepth 1 -type d | wc -l | tr -d ' ')" == 1 ]] || exit 74
    ls -A "$TMPDIR" | sort > "$ROOT_LISTING"
    printf '%s\n' "$LOCUS_HOME" > "$PROBE_FILE"
    ;;
  binding) ;;
  --version) ;;
  quickstart) exit 42 ;;
  *) exit 92 ;;
esac
EOF
chmod +x "$FIXTURE/target/debug/"*

status=0
env -u DOGFOOD_USE_REAL_HOME -u DOGFOOD_SELF_TEST -u DOGFOOD_APPLY -u DOGFOOD_PACK \
    -u DOGFOOD_CLIENT -u DOGFOOD_SKIP_HUB_SMOKE -u DOGFOOD_CLIENTS -u DOGFOOD_MT \
    -u LOCUS_DOGFOOD_REQUIRE_CLIENTS -u LOCUS_HOME -u LOCUS_SESSION_ID \
    BASH_ENV= ENV= \
    TMPDIR="$SCRATCH" \
    PROBE_FILE="$TEST_ROOT/probe" \
    ROOT_LISTING="$TEST_ROOT/listing" \
    MKTEMP_TEMPLATES="$TEST_ROOT/templates" \
    bash "$FIXTURE/scripts/dogfood.sh" >"$TEST_ROOT/stdout" 2>"$TEST_ROOT/stderr" || status=$?

[[ "$status" -eq 42 ]] \
  || fail "run must reach the controlled quickstart stop (status=$status): $(tail -n 5 "$TEST_ROOT/stderr")"

# Selection: the only template ever requested sits directly beneath the
# supplied root.
[[ -f "$TEST_ROOT/templates" ]] || fail "mktemp was never invoked"
[[ "$(wc -l < "$TEST_ROOT/templates" | tr -d ' ')" == 1 ]] \
  || fail "expected exactly one allocation: $(cat "$TEST_ROOT/templates")"
[[ "$(cat "$TEST_ROOT/templates")" == "$SCRATCH/locus-dogfood.XXXXXX" ]] \
  || fail "selected template was not beneath the supplied TMPDIR: $(cat "$TEST_ROOT/templates")"

# Creation: the recorded home is a single mktemp component directly inside it.
[[ -f "$TEST_ROOT/probe" ]] || fail "locus init never ran"
home="$(cat "$TEST_ROOT/probe")"
[[ "${home%/*}" == "$SCRATCH" ]] \
  || fail "home was not created directly beneath TMPDIR: $home"
case "${home##*/}" in
  locus-dogfood.*) ;;
  *) fail "unexpected home leaf: $home" ;;
esac
grep -Fq "isolated LOCUS_HOME=$home" "$TEST_ROOT/stdout" \
  || fail "script did not confirm the isolated home it selected"

# The controlled root held only the sentinel plus the new home at init time.
expected_listing="$(printf '%s\n%s\n' "${home##*/}" sentinel | sort)"
[[ "$(cat "$TEST_ROOT/listing")" == "$expected_listing" ]] \
  || fail "TMPDIR held unexpected entries at init: $(cat "$TEST_ROOT/listing")"

# Cleanup: the exit trap removes the home and nothing else.
[[ ! -e "$home" && ! -L "$home" ]] || fail "home was not removed on exit: $home"
[[ "$(ls -A "$SCRATCH")" == sentinel && "$(cat "$SCRATCH/sentinel")" == keep ]] \
  || fail "cleanup disturbed unrelated content in TMPDIR"

printf 'PASS: dogfood home was created beneath the supplied TMPDIR and cleaned up\n'
