#!/usr/bin/env bash
# Run: bash scripts/dogfood.test.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/locus-dogfood-test.XXXXXX")"
trap 'rm -rf "$test_root"' EXIT

(
  source "$ROOT/scripts/dogfood.sh"
  mktemp() {
    printf '%s\n' "$*" > "$test_root/mktemp-called"
    printf 'injected allocation failure\n' >&2
    return 1
  }
  locus() { printf 'locus\n' >> "$test_root/unexpected"; return 1; }
  cd() { printf 'cd\n' >> "$test_root/unexpected"; return 1; }
  rm() { printf 'rm\n' >> "$test_root/unexpected"; return 1; }

  # Match the production caller: an OR-list suppresses Bash errexit in the probe.
  if mt_grant_probe; then
    printf 'FAIL: multi-tenant probe accepted an allocation failure\n' >&2
    exit 1
  fi
) 2>"$test_root/stderr"

[[ "$(cat "$test_root/mktemp-called")" == "-d ${TMPDIR:-/tmp}/locus-mt-dogfood.XXXXXX" ]] || {
  printf 'FAIL: unexpected multi-tenant allocation request\n' >&2; exit 1;
}
grep -q 'injected allocation failure' "$test_root/stderr" || {
  printf 'FAIL: allocation failure was not reported\n' >&2; exit 1;
}
[[ ! -e "$test_root/unexpected" ]] || {
  printf 'FAIL: probe ran commands after allocation failed\n' >&2; exit 1;
}
printf 'PASS: multi-tenant probe stops when directory creation fails\n'
