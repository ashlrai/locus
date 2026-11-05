#!/usr/bin/env bash
# Offline regressions for issue #169: an installation directory containing
# spaces, and destinations the installer cannot write.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
test_root=$(mktemp -d "${TMPDIR:-/tmp}/locus-install-spaces.XXXXXX")
trap 'rm -rf -- "$test_root"' EXIT

target=x86_64-unknown-linux-gnu
asset="locus-$target.tar.gz"
mkdir -p "$test_root/fixture/locus-$target" "$test_root/tools" "$test_root/home"
printf '#!/bin/sh\nprintf "locus fixture\\n"\n' > "$test_root/fixture/locus-$target/locus"
printf '#!/bin/sh\nprintf "mcp fixture\\n"\n' > "$test_root/fixture/locus-$target/locus-mcp"
tar -czf "$test_root/$asset" -C "$test_root/fixture" "locus-$target"
if command -v sha256sum >/dev/null 2>&1; then
    digest=$(sha256sum "$test_root/$asset")
else
    digest=$(shasum -a 256 "$test_root/$asset")
fi
printf '%s  %s\n' "${digest%% *}" "$asset" > "$test_root/$asset.sha256"

cat > "$test_root/tools/uname" <<'MOCK'
#!/bin/bash
case "$1" in
    -s) printf 'Linux\n' ;;
    -m) printf 'x86_64\n' ;;
    *) exit 1 ;;
esac
MOCK
cat > "$test_root/tools/curl" <<'MOCK'
#!/bin/bash
set -euo pipefail
out= url=
while (($#)); do
    case "$1" in
        --output) out=$2; shift 2 ;;
        --proto|--proto-redir|--connect-timeout|--max-time|--retry) shift 2 ;;
        https://*) url=$1; shift ;;
        *) shift ;;
    esac
done
case "$url" in
    "https://github.com/ashlrai/locus/releases/download/v1.2.3/$TEST_ASSET"|\
    "https://github.com/ashlrai/locus/releases/download/v1.2.3/$TEST_ASSET.sha256")
        cp "$TEST_ROOT/${url##*/}" "$out" ;;
    *) exit 1 ;;
esac
MOCK
chmod +x "$test_root/tools/uname" "$test_root/tools/curl"

passed=0
fail() { printf 'FAIL: %s\n' "$1" >&2; exit 1; }
ok() { passed=$((passed + 1)); printf 'ok %d - %s\n' "$passed" "$1"; }

destination="$test_root/install directory/nested bin"
TEST_ROOT="$test_root" TEST_ASSET="$asset" HOME="$test_root/home" \
    PATH="$test_root/tools:$PATH" bash "$root/scripts/install.sh" \
    --version v1.2.3 --bin-dir "$destination" > "$test_root/output"

for binary in locus locus-mcp; do
    [[ -x "$destination/$binary" ]] || fail "missing executable: $binary"
    cmp "$destination/$binary" "$test_root/fixture/locus-$target/$binary" \
        || fail "wrong $binary bytes"
done
[[ $("$destination/locus") == 'locus fixture' ]] || fail 'locus does not run'
[[ $("$destination/locus-mcp") == 'mcp fixture' ]] || fail 'locus-mcp does not run'
[[ ! -e "$test_root/install" ]] || fail 'destination path was word-split'
ok 'installs into a destination containing spaces'

# Issue #169: a destination that cannot be written reports failure, exits
# nonzero, and leaves no staging files in the destination or the temp area.
# chmod enforces this for unprivileged users; root bypasses permission bits,
# so as root a stub refuses paths inside the locked directories instead.
real_mktemp=$(command -v mktemp)
real_mkdir=$(command -v mkdir)
if [[ $(id -u) -eq 0 ]]; then
    cat > "$test_root/locked-guard" <<'MOCK'
#!/bin/bash
for arg in "$@"; do
    case "$arg" in
        "$TEST_LOCKED"/*)
            printf '%s: cannot create directory %s: Permission denied\n' "${0##*/}" "$arg" >&2
            exit 1 ;;
    esac
done
case "${0##*/}" in
    mktemp) exec "$TEST_REAL_MKTEMP" "$@" ;;
    *) exec "$TEST_REAL_MKDIR" "$@" ;;
esac
MOCK
    chmod +x "$test_root/locked-guard"
    ln -s "$test_root/locked-guard" "$test_root/tools/mktemp"
    ln -s "$test_root/locked-guard" "$test_root/tools/mkdir"
fi

unwritable_case() {
    local name=$1 locked=$2 bin_dir=$3 message=$4
    local tmp="$test_root/tmp-$name" status=0
    mkdir "$tmp" "$locked"
    chmod a-w "$locked"
    TEST_ROOT="$test_root" TEST_ASSET="$asset" TEST_LOCKED="$locked" \
        TEST_REAL_MKTEMP="$real_mktemp" TEST_REAL_MKDIR="$real_mkdir" \
        HOME="$test_root/home" TMPDIR="$tmp" PATH="$test_root/tools:$PATH" \
        bash "$root/scripts/install.sh" --version v1.2.3 --bin-dir "$bin_dir" \
        > "$test_root/$name.out" 2> "$test_root/$name.err" || status=$?
    [[ $status -ne 0 ]] || fail "$name: installer unexpectedly succeeded"
    grep -Fq "locus installer: $message" "$test_root/$name.err" \
        || fail "$name: missing installer report: $(cat "$test_root/$name.err")"
    if grep -Fq 'Installed locus and locus-mcp' "$test_root/$name.out"; then
        fail "$name: reported success"
    fi
    [[ -z "$(ls -A "$locked")" ]] || fail "$name: destination changed"
    [[ -z "$(ls -A "$tmp")" ]] || fail "$name: temporary files leaked"
    ok "$name"
}

# An existing destination without write permission refuses at staging.
unwritable_case 'unwritable-destination' "$test_root/locked dest" \
    "$test_root/locked dest" 'cannot create staging directory'
# A missing destination under an unwritable parent refuses at creation.
unwritable_case 'unwritable-parent' "$test_root/locked parent" \
    "$test_root/locked parent/new bin" 'cannot create install directory'

printf '%d installer tests passed; 0 failed.\n' "$passed"
