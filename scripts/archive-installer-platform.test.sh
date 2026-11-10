#!/usr/bin/env bash
# Verify platform rejection precedes even the latest-release lookup, offline.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
tmp=$(mktemp -d)
trap 'rm -rf -- "$tmp"' EXIT
mkdir -p "$tmp/tools" "$tmp/home"
cat > "$tmp/tools/uname" <<'MOCK'
#!/bin/bash
case "$1" in
  -s) printf 'Linux\n' ;;
  -m) printf 'aarch64\n' ;;
  *) exit 1 ;;
esac
MOCK
cat > "$tmp/tools/curl" <<'MOCK'
#!/bin/bash
: > "$TEST_CURL_MARKER"
exit 1
MOCK
chmod +x "$tmp/tools/uname" "$tmp/tools/curl"
status=0
HOME="$tmp/home" PATH="$tmp/tools:$PATH" TEST_CURL_MARKER="$tmp/curl-called" \
  bash "$root/scripts/install.sh" --bin-dir "$tmp/bin" >"$tmp/out" 2>"$tmp/err" || status=$?
[[ $status -ne 0 ]] || { printf 'installer unexpectedly succeeded\n' >&2; exit 1; }
grep -Fq 'locus installer: unsupported platform: Linux/aarch64; use a source install' "$tmp/err" \
  || { printf 'missing platform error\n' >&2; exit 1; }
[[ ! -e "$tmp/curl-called" && ! -e "$tmp/bin" ]] \
  || { printf 'download or installation attempted\n' >&2; exit 1; }
printf '1 installer platform test passed; 0 failed.\n'
