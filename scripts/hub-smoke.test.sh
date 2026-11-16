#!/usr/bin/env bash
# Run: bash scripts/hub-smoke.test.sh
# Exercise allocation failures and a complete smoke without touching real state.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
real_mktemp="$(command -v mktemp)"
fixture="$($real_mktemp -d "${TMPDIR:-/tmp}/hub-smoke-test.XXXXXX")"
trap 'rm -rf -- "$fixture"' EXIT
mkdir -p "$fixture/bin" "$fixture/scratch with spaces"
printf 'keep' > "$fixture/scratch with spaces/sentinel"
cat > "$fixture/bin/mktemp" <<'STUB'
#!/usr/bin/env bash
if [[ -n "${CHECK_ARGUMENTS:-}" ]]; then
  if ! [[ ( "$#" -eq 2 && "$1" == -d ) || "$#" -eq 1 ]]; then
    exit 75
  fi
  [[ "${!#}" == "$TMPDIR"/* ]] || exit 75
fi
printf '%s\n' "${!#}" >> "$ATTEMPTS"
if [[ "${!#}" == */"$FAIL_TEMPLATE" ]]; then
  exit 1
fi
exec "$REAL_MKTEMP" "$@"
STUB
chmod +x "$fixture/bin/mktemp"
real_rm="$(command -v rm)"
cat > "$fixture/bin/rm" <<'STUB'
#!/usr/bin/env bash
if [[ -n "${CHECK_ARGUMENTS:-}" ]]; then
  [[ "$#" -eq 5 && "$1" == -rf && "$2" == -- ]] || exit 76
  shift 2
  for path in "$@"; do
    [[ "${path%/*}" == "$TMPDIR" && -e "$path" ]] || exit 76
  done
  printf 'checked\n' > "$CLEANUP_RECORD"
  exec "$REAL_RM" -rf -- "$@"
fi
exec /bin/rm "$@"
STUB
chmod +x "$fixture/bin/rm"

# Record any attempt to enter the smoke setup or workload after allocation.
cat > "$fixture/allocation-hook" <<'HOOK'
od() { : > "$SMOKE_REACHED"; }
jq() { : > "$SMOKE_REACHED"; }
cargo() { : > "$SMOKE_REACHED"; }
locus() { : > "$SMOKE_REACHED"; }
HOOK

for case_number in 1 2 3; do
  case "$case_number" in
    1) template='locus-hub-smoke.XXXXXX'; diagnostic='temporary home' ;;
    2) template='locus-hub-proj.XXXXXX'; diagnostic='temporary project directory' ;;
    3) template='locus-hub-smoke-err.XXXXXX'; diagnostic='temporary error file' ;;
  esac
  : > "$fixture/attempts"
  rm -f "$fixture/smoke-reached"
  status=0
  env BASH_ENV="$fixture/allocation-hook" SMOKE_REACHED="$fixture/smoke-reached" \
    HOME="$fixture" TMPDIR="$fixture/scratch with spaces" \
    PATH="$fixture/bin:$PATH" REAL_MKTEMP="$real_mktemp" \
    FAIL_TEMPLATE="$template" ATTEMPTS="$fixture/attempts" \
    bash "$root/scripts/hub-smoke.sh" > "$fixture/stdout" 2> "$fixture/stderr" || status=$?
  if [[ "$status" -eq 0 ]] || ! grep -Fxq "error: hub-smoke could not create $diagnostic" "$fixture/stderr" \
    || [[ -s "$fixture/stdout" || -e "$fixture/smoke-reached" ]] \
    || [[ $(wc -l < "$fixture/attempts") -ne "$case_number" ]] \
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

# Inspect all three allocations at their first use, before the EXIT trap removes them.
cat > "$fixture/hook" <<'HOOK'
jq() { :; }
cargo() { :; }
locus() {
  if [[ "${1:-}" == init ]]; then
    [[ -d "$SMOKE_HOME" && ! -L "$SMOKE_HOME" &&
       -d "$SMOKE_PROJ" && ! -L "$SMOKE_PROJ" &&
       -f "$SMOKE_ERR" && ! -L "$SMOKE_ERR" ]] || exit 74
    for path in "$SMOKE_HOME" "$SMOKE_PROJ" "$SMOKE_ERR"; do
      [[ "${path%/*}" == "$TMPDIR" ]] || exit 74
    done
    [[ "${SMOKE_HOME##*/}" == locus-hub-smoke.* &&
       "${SMOKE_PROJ##*/}" == locus-hub-proj.* &&
       "${SMOKE_ERR##*/}" == locus-hub-smoke-err.* ]] || exit 74
    printf '%s\n' "$SMOKE_ERR" > "$RECORD"
    exit 73
  fi
}
HOOK
status=0
env BASH_ENV="$fixture/hook" HOME="$fixture" TMPDIR="$fixture/scratch with spaces" \
  PATH="$fixture/bin:$PATH" REAL_MKTEMP="$real_mktemp" FAIL_TEMPLATE=none \
  ATTEMPTS="$fixture/attempts" RECORD="$fixture/record" CHECK_ARGUMENTS=1 \
  REAL_RM="$real_rm" CLEANUP_RECORD="$fixture/cleanup-record" \
  bash "$root/scripts/hub-smoke.sh" > "$fixture/stdout" 2> "$fixture/stderr" || status=$?
[[ "$status" -eq 73 ]] || { echo 'FAIL: temporary paths were not observed under TMPDIR at init' >&2; exit 1; }
[[ -f "$fixture/cleanup-record" ]] || { echo 'FAIL: cleanup did not receive three separate paths' >&2; exit 1; }
IFS= read -r error_file < "$fixture/record"
[[ "${error_file%/*}" == "$fixture/scratch with spaces" && \
   "${error_file##*/}" == locus-hub-smoke-err.* && ! -e "$error_file" ]] || {
  echo 'FAIL: error file was not created under TMPDIR and cleaned up' >&2; exit 1;
}
[[ $(find "$fixture/scratch with spaces" -mindepth 1 -maxdepth 1 | wc -l) -eq 1 && \
   $(cat "$fixture/scratch with spaces/sentinel") == keep ]] || {
  echo 'FAIL: cleanup changed the supplied TMPDIR' >&2; exit 1;
}
echo 'ok: all temporary paths use the supplied TMPDIR and are cleaned up'

# Interrupt a running smoke after all allocations, without invoking real services.
cat > "$fixture/interrupt-hook" <<'HOOK'
jq() { :; }
cargo() { :; }
locus() {
  if [[ "${1:-}" == init ]]; then
    [[ -d "$SMOKE_HOME" && -d "$SMOKE_PROJ" && -f "$SMOKE_ERR" ]] || exit 74
    printf '%s\n' "$SMOKE_HOME" "$SMOKE_PROJ" "$SMOKE_ERR" > "$INTERRUPT_RECORD"
    while :; do :; done
  fi
}
HOOK
# Python launches a foreground-like child so SIGINT is not inherited as ignored
# from a shell background job. The timeout also prevents a broken trap hanging CI.
env BASH_ENV="$fixture/interrupt-hook" HOME="$fixture" TMPDIR="$fixture/scratch with spaces" \
  PATH="$fixture/bin:$PATH" REAL_MKTEMP="$real_mktemp" FAIL_TEMPLATE=none \
  ATTEMPTS="$fixture/attempts" INTERRUPT_RECORD="$fixture/interrupt-record" \
  python3 - "$root/scripts/hub-smoke.sh" "$fixture/interrupt-record" <<'PYTHON'
import os
import signal
import subprocess
import sys
import time

child = subprocess.Popen(['bash', sys.argv[1]], stdout=subprocess.DEVNULL,
                         stderr=subprocess.DEVNULL, start_new_session=True)
try:
    deadline = time.monotonic() + 10
    while not os.path.exists(sys.argv[2]):
        if child.poll() is not None or time.monotonic() >= deadline:
            raise RuntimeError('smoke did not reach the interrupt point')
        time.sleep(0.01)
    child.send_signal(signal.SIGINT)
    status = child.wait(timeout=10)
    if status not in (128 + signal.SIGINT, -signal.SIGINT):
        raise RuntimeError(f'unexpected interrupted exit status: {status}')
finally:
    if child.poll() is None:
        child.kill()
        child.wait()
PYTHON
[[ $(wc -l < "$fixture/interrupt-record") -eq 3 ]] || {
  echo 'FAIL: interrupt test did not record all temporary paths' >&2; exit 1;
}
while IFS= read -r path; do
  [[ "${path%/*}" == "$fixture/scratch with spaces" && ! -e "$path" ]] || {
    echo 'FAIL: interrupted smoke left a temporary path behind' >&2; exit 1;
  }
done < "$fixture/interrupt-record"
[[ $(find "$fixture/scratch with spaces" -mindepth 1 -maxdepth 1 | wc -l) -eq 1 && \
   $(cat "$fixture/scratch with spaces/sentinel") == keep ]] || {
  echo 'FAIL: interrupt cleanup changed the supplied TMPDIR' >&2; exit 1;
}
echo 'ok: interrupt cleans all temporary paths'

# Run the complete smoke against real binaries with the same spaced TMPDIR.
# Keep output private: the smoke exercises credential-adjacent commands.
: > "$fixture/attempts"
status=0
env BASH_ENV= HOME="$fixture" TMPDIR="$fixture/scratch with spaces" \
  PATH="$fixture/bin:$PATH" REAL_MKTEMP="$real_mktemp" FAIL_TEMPLATE=none \
  ATTEMPTS="$fixture/attempts" \
  bash "$root/scripts/hub-smoke.sh" > "$fixture/stdout" 2> "$fixture/stderr" || status=$?
if [[ "$status" -ne 0 ]] || ! grep -Fxq '== hub-smoke OK ==' "$fixture/stdout" \
  || [[ $(wc -l < "$fixture/attempts") -ne 3 ]] \
  || [[ $(find "$fixture/scratch with spaces" -mindepth 1 -maxdepth 1 | wc -l) -ne 1 ]] \
  || [[ $(cat "$fixture/scratch with spaces/sentinel") != keep ]]; then
  echo 'FAIL: complete smoke with spaced TMPDIR or cleanup' >&2
  exit 1
fi
echo 'ok: complete smoke with spaced TMPDIR and cleanup'
