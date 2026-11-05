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
