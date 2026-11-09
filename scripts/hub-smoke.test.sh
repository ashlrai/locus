#!/usr/bin/env bash
# Run: bash scripts/hub-smoke.test.sh
# Exercise each allocation failure without starting Locus or touching real state.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
real_mktemp="$(command -v mktemp)"
fixture="$($real_mktemp -d "${TMPDIR:-/tmp}/hub-smoke-test.XXXXXX")"
trap 'rm -rf -- "$fixture"' EXIT
mkdir -p "$fixture/bin" "$fixture/scratch with spaces"
printf 'keep' > "$fixture/scratch with spaces/sentinel"
cat > "$fixture/bin/mktemp" <<'STUB'
#!/usr/bin/env bash
printf '%s\n' "${!#}" >> "$ATTEMPTS"
if [[ "${!#}" == */"$FAIL_TEMPLATE" ]]; then
  exit 1
fi
exec "$REAL_MKTEMP" "$@"
STUB
chmod +x "$fixture/bin/mktemp"

for case_number in 1 2 3; do
  case "$case_number" in
    1) template='locus-hub-smoke.XXXXXX'; diagnostic='temporary home' ;;
    2) template='locus-hub-proj.XXXXXX'; diagnostic='temporary project directory' ;;
    3) template='locus-hub-smoke-err.XXXXXX'; diagnostic='temporary error file' ;;
  esac
  : > "$fixture/attempts"
  status=0
  env BASH_ENV= HOME="$fixture" TMPDIR="$fixture/scratch with spaces" \
    PATH="$fixture/bin:$PATH" REAL_MKTEMP="$real_mktemp" \
    FAIL_TEMPLATE="$template" ATTEMPTS="$fixture/attempts" \
    bash "$root/scripts/hub-smoke.sh" > "$fixture/stdout" 2> "$fixture/stderr" || status=$?
  if [[ "$status" -eq 0 ]] || ! grep -Fxq "error: hub-smoke could not create $diagnostic" "$fixture/stderr" \
    || [[ -s "$fixture/stdout" ]] || [[ $(wc -l < "$fixture/attempts") -ne "$case_number" ]] \
    || [[ $(find "$fixture/scratch with spaces" -mindepth 1 -maxdepth 1 | wc -l) -ne 1 ]] \
    || [[ $(cat "$fixture/scratch with spaces/sentinel") != keep ]]; then
    printf 'FAIL: allocation failure case %s\n' "$case_number" >&2
    exit 1
  fi
  printf 'ok: allocation failure case %s\n' "$case_number"
done

# Exercise the real mktemp against a directory without write permission.
# The hook records any attempt to reach the smoke prerequisites or checks.
mkdir "$fixture/unwritable"
cat > "$fixture/no-smoke-hook" <<'HOOK'
jq() { : > "$SMOKE_REACHED"; }
cargo() { : > "$SMOKE_REACHED"; }
locus() { : > "$SMOKE_REACHED"; }
HOOK
chmod 500 "$fixture/unwritable"
status=0
env BASH_ENV="$fixture/no-smoke-hook" HOME="$fixture" TMPDIR="$fixture/unwritable" \
  SMOKE_REACHED="$fixture/smoke-reached" \
  bash "$root/scripts/hub-smoke.sh" > "$fixture/stdout" 2> "$fixture/stderr" || status=$?
chmod 700 "$fixture/unwritable"
if [[ "$status" -eq 0 ]] || ! grep -Fxq 'error: hub-smoke could not create temporary home' "$fixture/stderr" \
  || [[ -s "$fixture/stdout" || -e "$fixture/smoke-reached" ]] \
  || [[ -n $(find "$fixture/unwritable" -mindepth 1 -print -quit) ]]; then
  echo 'FAIL: unwritable TMPDIR did not stop before smoke checks' >&2
  exit 1
fi
echo 'ok: unwritable TMPDIR stops before smoke checks'

# Inspect the real file at its first use, before the EXIT trap removes it.
cat > "$fixture/hook" <<'HOOK'
jq() { :; }
cargo() { :; }
locus() {
  if [[ "${1:-}" == init ]]; then
    [[ -f "$SMOKE_ERR" && ! -L "$SMOKE_ERR" ]] || exit 74
    printf '%s\n' "$SMOKE_ERR" > "$RECORD"
    exit 73
  fi
}
HOOK
status=0
env BASH_ENV="$fixture/hook" HOME="$fixture" TMPDIR="$fixture/scratch with spaces" \
  PATH="$fixture/bin:$PATH" REAL_MKTEMP="$real_mktemp" FAIL_TEMPLATE=none \
  ATTEMPTS="$fixture/attempts" RECORD="$fixture/record" \
  bash "$root/scripts/hub-smoke.sh" > "$fixture/stdout" 2> "$fixture/stderr" || status=$?
[[ "$status" -eq 73 ]] || { echo 'FAIL: error file was not observed at init' >&2; exit 1; }
IFS= read -r error_file < "$fixture/record"
[[ "${error_file%/*}" == "$fixture/scratch with spaces" && \
   "${error_file##*/}" == locus-hub-smoke-err.* && ! -e "$error_file" ]] || {
  echo 'FAIL: error file was not created under TMPDIR and cleaned up' >&2; exit 1;
}
[[ $(find "$fixture/scratch with spaces" -mindepth 1 -maxdepth 1 | wc -l) -eq 1 && \
   $(cat "$fixture/scratch with spaces/sentinel") == keep ]] || {
  echo 'FAIL: cleanup changed the supplied TMPDIR' >&2; exit 1;
}
echo 'ok: error file uses the supplied TMPDIR and is cleaned up'
