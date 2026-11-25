#!/usr/bin/env bash
# Offline regressions for cleanup on allocation failure and interruption.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/locus-hub-test.XXXXXX")"
trap 'rm -rf -- "$test_root"' EXIT
mkdir -p "$test_root/tmp with spaces" "$test_root/home"

# Fail the first allocation before any integration workload can run.
cat > "$test_root/first-failure-shim.sh" <<'SHIM'
jq() { return 0; }
node() { : > "$TEST_ROOT/workload-started"; return 1; }
mktemp() {
  case "$2" in
    */locus-hub-int.XXXXXX) : > "$TEST_ROOT/first-attempt"; return 1 ;;
    *) : > "$TEST_ROOT/unexpected-allocation"; return 1 ;;
  esac
}
SHIM

status=0
HOME="$test_root/home" TMPDIR="$test_root/tmp with spaces" TEST_ROOT="$test_root" \
  BASH_ENV="$test_root/first-failure-shim.sh" bash "$ROOT/scripts/hub-integration-test.sh" \
  >"$test_root/stdout" 2>"$test_root/stderr" || status=$?
if [[ $status -eq 0 || ! -f "$test_root/first-attempt" || -e "$test_root/unexpected-allocation" || -e "$test_root/workload-started" ]]; then
  echo 'FAIL: first allocation failure did not stop before the integration workload' >&2
  exit 1
fi
if [[ -n "$(find "$test_root/tmp with spaces" -mindepth 1 -print -quit)" ]]; then
  echo 'FAIL: first allocation failure left temporary paths behind' >&2
  exit 1
fi
echo 'ok: first directory allocation failure stops without leaving temporary paths'

# Real mktemp must fail against an unwritable parent before any integration
# commands run. As in install-test.sh, emulate permission denial for root,
# which bypasses directory mode bits; ordinary users exercise the real failure.
mkdir "$test_root/unwritable parent"
printf '%s\n' keep > "$test_root/unwritable parent/sentinel"
cat > "$test_root/unwritable-shim.sh" <<'SHIM'
jq() { printf 'jq\n' >> "$TEST_ROOT/unwritable-workload"; return 91; }
node() { printf 'node\n' >> "$TEST_ROOT/unwritable-workload"; return 91; }
cargo() { printf 'cargo\n' >> "$TEST_ROOT/unwritable-workload"; return 91; }
locus() { printf 'locus\n' >> "$TEST_ROOT/unwritable-workload"; return 91; }
mktemp() {
  printf '%s\n' "$*" >> "$TEST_ROOT/unwritable-attempts"
  if [[ $EUID -eq 0 && "$2" == "$TMPDIR/"* ]]; then
    printf 'mktemp: Permission denied\n' >&2
    return 1
  fi
  command mktemp "$@"
}
SHIM

# Restore permissions even if the test is interrupted, so its own fixture
# cleanup can remove the sentinel and parent.
trap 'chmod 700 "$test_root/unwritable parent"; rm -rf -- "$test_root"' EXIT
chmod 500 "$test_root/unwritable parent"
status=0
HOME="$test_root/home" TMPDIR="$test_root/unwritable parent" TEST_ROOT="$test_root" \
  LC_ALL=C BASH_ENV="$test_root/unwritable-shim.sh" bash "$ROOT/scripts/hub-integration-test.sh" \
  >"$test_root/stdout" 2>"$test_root/stderr" || status=$?
chmod 700 "$test_root/unwritable parent"
if [[ $status -eq 0 || -s "$test_root/stdout" || -e "$test_root/unwritable-workload" ]] ||
  ! grep -Fq 'mktemp:' "$test_root/stderr" ||
  ! grep -Fq 'Permission denied' "$test_root/stderr" ||
  [[ "$(cat "$test_root/unwritable-attempts")" != "-d $test_root/unwritable parent/locus-hub-int.XXXXXX" ]]; then
  echo 'FAIL: unwritable parent did not stop at the first allocation' >&2
  exit 1
fi
if [[ "$(ls -A "$test_root/unwritable parent")" != sentinel ||
      "$(cat "$test_root/unwritable parent/sentinel")" != keep ||
      -n "$(ls -A "$test_root/home")" ||
      -n "$(ls -A "$test_root/tmp with spaces")" ]]; then
  echo 'FAIL: unwritable parent failure changed the fixtures or left artifacts' >&2
  exit 1
fi
echo 'ok: unwritable temporary parent stops without running integration commands or leaving artifacts'

# BASH_ENV supplies the shim to the real script, including its command
# substitutions. Record the first allocation outside the cleanup target.
cat > "$test_root/shim.sh" <<'SHIM'
jq() { return 0; }
node() { return 0; }
mktemp() {
  case "$2" in
    */locus-hub-int.XXXXXX)
      local dir
      dir="$(command mktemp "$@")" || return
      printf '%s\n' "$dir" > "$TEST_ROOT/first-dir"
      printf '%s\n' "$dir"
      ;;
    */locus-hub-int-proj.XXXXXX)
      printf '%s\n' reached > "$TEST_ROOT/second-attempt"
      return 1
      ;;
    *) return 1 ;;
  esac
}
SHIM

status=0
HOME="$test_root/home" TMPDIR="$test_root/tmp with spaces" TEST_ROOT="$test_root" \
  BASH_ENV="$test_root/shim.sh" bash "$ROOT/scripts/hub-integration-test.sh" \
  >"$test_root/stdout" 2>"$test_root/stderr" || status=$?

if [[ $status -eq 0 || ! -s "$test_root/first-dir" || ! -f "$test_root/second-attempt" ]]; then
  echo 'FAIL: expected the second directory allocation to fail' >&2
  exit 1
fi
IFS= read -r first_dir < "$test_root/first-dir"
if [[ "$first_dir" != "$test_root/tmp with spaces"/locus-hub-int.* || -e "$first_dir" ]]; then
  echo 'FAIL: first smoke directory was outside TMPDIR or not removed' >&2
  exit 1
fi
echo 'ok: second directory allocation failure cleans up the first directory'

# Interrupt the real script after both allocations, without starting any
# external services. The node shim runs in the script's shell, so $$ is the
# process receiving the signal (background shells can inherit ignored SIGINT).
cat > "$test_root/interrupt-shim.sh" <<'SHIM'
jq() { return 0; }
mktemp() {
  local dir
  dir="$(command mktemp "$@")" || return
  case "$2" in
    */locus-hub-int.XXXXXX) printf '%s\n' "$dir" > "$TEST_ROOT/interrupt-home" ;;
    */locus-hub-int-proj.XXXXXX) printf '%s\n' "$dir" > "$TEST_ROOT/interrupt-project" ;;
    *) return 1 ;;
  esac
  printf '%s\n' "$dir"
}
node() {
  # Model a scan listing left behind when interruption occurs.
  [[ -d "$(cat "$TEST_ROOT/interrupt-home")" && -d "$(cat "$TEST_ROOT/interrupt-project")" ]] || return 1
  : > "$TMPDIR/.locus-hub-int-scan.$$.fixture"
  : > "$TEST_ROOT/interrupt-ready"
  kill -s "$TEST_SIGNAL" "$$"
  return 1
}
SHIM

for signal in INT TERM; do
  rm -f -- "$test_root/interrupt-home" "$test_root/interrupt-project" "$test_root/interrupt-ready"
  status=0
  HOME="$test_root/home" TMPDIR="$test_root/tmp with spaces" TEST_ROOT="$test_root" \
    TEST_SIGNAL="$signal" BASH_ENV="$test_root/interrupt-shim.sh" \
    bash "$ROOT/scripts/hub-integration-test.sh" \
    >"$test_root/stdout" 2>"$test_root/stderr" || status=$?
  expected_status=130
  [[ $signal == TERM ]] && expected_status=143
  if [[ $status -ne $expected_status || ! -s "$test_root/interrupt-home" || ! -s "$test_root/interrupt-project" || ! -f "$test_root/interrupt-ready" ]]; then
    echo "FAIL: $signal did not interrupt after both allocations" >&2
    exit 1
  fi
  IFS= read -r smoke_home < "$test_root/interrupt-home"
  IFS= read -r smoke_project < "$test_root/interrupt-project"
  if [[ "$smoke_home" != "$test_root/tmp with spaces"/locus-hub-int.* || \
        "$smoke_project" != "$test_root/tmp with spaces"/locus-hub-int-proj.* || \
        -e "$smoke_home" || -e "$smoke_project" ]] || \
    compgen -G "$test_root/tmp with spaces/.locus-hub-int-scan.*" >/dev/null; then
    echo "FAIL: $signal used the wrong TMPDIR or left temporary paths behind" >&2
    exit 1
  fi
  echo "ok: $signal removes both smoke directories and scan listings"
done

# Stop after both real allocations, exercising the EXIT trap entirely offline.
cat > "$test_root/spaces-shim.sh" <<'SHIM'
jq() { return 0; }
node() { return 42; }
mktemp() {
  local dir
  dir="$(command mktemp "$@")" || return
  printf '%s\n' "$dir" >> "$TEST_ROOT/allocated-dirs"
  printf '%s\n' "$dir"
}
SHIM

status=0
HOME="$test_root/home" TMPDIR="$test_root/tmp with spaces" TEST_ROOT="$test_root" \
  BASH_ENV="$test_root/spaces-shim.sh" bash "$ROOT/scripts/hub-integration-test.sh" \
  >"$test_root/spaces-stdout" 2>"$test_root/spaces-stderr" || status=$?

if [[ $status -ne 42 || ! -f "$test_root/allocated-dirs" ]]; then
  echo 'FAIL: expected both allocations before the offline stop' >&2
  exit 1
fi
home_count=0
project_count=0
while IFS= read -r dir; do
  case "$dir" in
    "$test_root/tmp with spaces"/locus-hub-int.*) home_count=$((home_count + 1)) ;;
    "$test_root/tmp with spaces"/locus-hub-int-proj.*) project_count=$((project_count + 1)) ;;
    *) echo 'FAIL: smoke directory allocated outside the spaced parent' >&2; exit 1 ;;
  esac
  if [[ -e "$dir" ]]; then
    echo 'FAIL: smoke directory under spaced parent was not removed' >&2
    exit 1
  fi
done < "$test_root/allocated-dirs"
if [[ $home_count -ne 1 || $project_count -ne 1 ]] || [[ -n "$(ls -A "$test_root/tmp with spaces")" ]]; then
  echo 'FAIL: expected two allocations and an empty spaced parent' >&2
  exit 1
fi
echo 'ok: both smoke directories under a spaced parent are cleaned up'
