#!/usr/bin/env bash
# Exercise the actual smoke script when temporary-path allocation fails.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
REAL_MKTEMP="$(command -v mktemp)"
TEST_DIR="$("$REAL_MKTEMP" -d "${TMPDIR:-/tmp}/locus-hub-cleanup-test.XXXXXX")"
trap 'rm -rf -- "$TEST_DIR"' EXIT
mkdir "$TEST_DIR/bin" "$TEST_DIR/tmp"

cat >"$TEST_DIR/bin/mktemp" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
count=0
[[ ! -f "$CLEANUP_COUNT" ]] || read -r count <"$CLEANUP_COUNT"
count=$((count + 1))
printf '%s\n' "$count" >"$CLEANUP_COUNT"
if [[ "$count" -eq "$CLEANUP_FAIL_AT" ]]; then
  exit 73
fi
path="$("$CLEANUP_REAL_MKTEMP" "$@")"
printf '%s\n' "$path" >>"$CLEANUP_CREATED"
printf '%s\n' "$path"
EOF
chmod +x "$TEST_DIR/bin/mktemp"

for fail_at in 2 3; do
  count_file="$TEST_DIR/count"
  created_file="$TEST_DIR/created"
  rm -f -- "$count_file" "$created_file"
  status=0
  PATH="$TEST_DIR/bin:$PATH" TMPDIR="$TEST_DIR/tmp" \
    CLEANUP_COUNT="$count_file" CLEANUP_CREATED="$created_file" \
    CLEANUP_FAIL_AT="$fail_at" CLEANUP_REAL_MKTEMP="$REAL_MKTEMP" \
    bash "$ROOT/scripts/hub-smoke.sh" >"$TEST_DIR/output" 2>&1 || status=$?
  if [[ "$status" -ne 73 || ! -f "$count_file" || $(<"$count_file") -ne "$fail_at" ]]; then
    echo "FAIL  hub-smoke cleanup: allocation failure $fail_at was not reached" >&2
    exit 1
  fi
  created=0
  while IFS= read -r path; do
    created=$((created + 1))
    if [[ -e "$path" ]]; then
      echo "FAIL  hub-smoke cleanup: temporary path survived allocation failure $fail_at" >&2
      exit 1
    fi
  done <"$created_file"
  if [[ "$created" -ne $((fail_at - 1)) ]]; then
    echo "FAIL  hub-smoke cleanup: unexpected allocation count" >&2
    exit 1
  fi
done
echo "ok    hub-smoke cleans temporary paths after early allocation failures"
