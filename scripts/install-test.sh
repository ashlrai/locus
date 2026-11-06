#!/usr/bin/env bash
# Offline regressions for issue #169: an installation directory containing
# spaces, destinations the installer cannot write, an unsupported operating
# system, a failed download, and malformed release archives that must not
# replace existing executables.
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
    "https://github.com/ashlrai/locus/releases/download/v1.2.3/$TEST_ASSET")
        [[ ${TEST_FAIL_DOWNLOAD:-} != archive ]] || exit 1
        cp "${INSTALL_TEST_ARCHIVE:-$TEST_ROOT/${url##*/}}" "$out" ;;
    "https://github.com/ashlrai/locus/releases/download/v1.2.3/$TEST_ASSET.sha256")
        [[ ${TEST_FAIL_DOWNLOAD:-} != checksum ]] || exit 1
        cp "${INSTALL_TEST_SIDECAR:-$TEST_ROOT/${url##*/}}" "$out" ;;
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

# Issue #169: an unsupported operating system reports failure, exits nonzero,
# and is refused before any download or install attempt. The stubbed curl
# leaves a sentinel if invoked; its absence proves the refusal is pre-network.
unsupported_case() {
    local name=$1 os=$2 arch=$3
    local case_dir="$test_root/$name" tmp="$test_root/tmp-$name" status=0
    local tools="$case_dir/tools" dest="$case_dir/dest dir"
    mkdir -p "$tools" "$tmp"
    cat > "$tools/uname" <<'MOCK'
#!/bin/bash
case "$1" in
    -s) printf '%s\n' "$TEST_OS" ;;
    -m) printf '%s\n' "$TEST_ARCH" ;;
    *) exit 1 ;;
esac
MOCK
    cat > "$tools/curl" <<'MOCK'
#!/bin/bash
printf 'curl should not run on an unsupported platform\n' > "$TEST_CURL_INVOKED"
exit 1
MOCK
    chmod +x "$tools/uname" "$tools/curl"
    TEST_OS="$os" TEST_ARCH="$arch" TEST_CURL_INVOKED="$case_dir/curl-invoked" \
        HOME="$test_root/home" TMPDIR="$tmp" PATH="$tools:$PATH" \
        bash "$root/scripts/install.sh" --version v1.2.3 --bin-dir "$dest" \
        > "$case_dir/out" 2> "$case_dir/err" || status=$?
    [[ $status -ne 0 ]] || fail "$name: installer unexpectedly succeeded"
    grep -Fq "locus installer: unsupported platform: $os/$arch; use a source install" \
        "$case_dir/err" || fail "$name: missing installer report: $(cat "$case_dir/err")"
    if grep -Fq 'Installed locus and locus-mcp' "$case_dir/out"; then
        fail "$name: reported success"
    fi
    [[ ! -e "$case_dir/curl-invoked" ]] || fail "$name: download attempted"
    [[ ! -e "$dest" ]] || fail "$name: destination created"
    [[ -z "$(ls -A "$tmp")" ]] || fail "$name: temporary files leaked"
    ok "$name"
}

unsupported_case 'unsupported-os-freebsd' FreeBSD amd64

# Issue #169: a failed download reports failure, exits nonzero, and leaves no
# installed executable or staging file in the destination or the temp area.
# TEST_FAIL_DOWNLOAD names the stubbed request the mock curl refuses.
failed_download_case() {
    local name=$1 which=$2 message=$3
    local dest="$test_root/failed-$name" tmp="$test_root/tmp-failed-$name" status=0
    mkdir "$dest" "$tmp"
    # TEST_LOCKED points at a path nothing uses so the root path guard stays
    # inert; the real tools keep mktemp/mkdir working for this case.
    TEST_ROOT="$test_root" TEST_ASSET="$asset" TEST_FAIL_DOWNLOAD="$which" \
        TEST_LOCKED="$test_root/unused-locked" \
        TEST_REAL_MKTEMP="$real_mktemp" TEST_REAL_MKDIR="$real_mkdir" \
        HOME="$test_root/home" TMPDIR="$tmp" PATH="$test_root/tools:$PATH" \
        bash "$root/scripts/install.sh" --version v1.2.3 --bin-dir "$dest" \
        > "$test_root/failed-$name.out" 2> "$test_root/failed-$name.err" || status=$?
    [[ $status -ne 0 ]] || fail "$name: installer unexpectedly succeeded"
    grep -Fq "locus installer: $message" "$test_root/failed-$name.err" \
        || fail "$name: missing installer report: $(cat "$test_root/failed-$name.err")"
    if grep -Fq 'Installed locus and locus-mcp' "$test_root/failed-$name.out"; then
        fail "$name: reported success"
    fi
    [[ -z "$(ls -A "$dest")" ]] || fail "$name: destination changed"
    [[ -z "$(ls -A "$tmp")" ]] || fail "$name: temporary files leaked"
    ok "$name"
}

failed_download_case 'archive-download-failure' archive \
    'archive download failed; nothing installed'
failed_download_case 'checksum-download-failure' checksum \
    'checksum download failed; nothing installed'

# Issue #169: a malformed archive fails closed even when its checksum sidecar
# matches the served bytes — the installer exits nonzero, never reports
# success, leaves pre-existing executables untouched, and cleans its temporary
# area. INSTALL_TEST_ARCHIVE and INSTALL_TEST_SIDECAR point the mock curl at
# per-case fixtures; the names differ from test-install.sh's exported
# TEST_ARCHIVE so that harness cannot leak an override into this script.
malformed="$test_root/malformed"
mkdir -p "$malformed"

# Bytes that never formed a gzip stream.
printf 'this is not a release archive\n' > "$malformed/not-gzip.tar.gz"

# A real archive cut short mid-stream.
head -c $(( $(wc -c < "$test_root/$asset") / 2 )) "$test_root/$asset" \
    > "$malformed/truncated.tar.gz"

# A valid archive missing the locus-mcp member.
mkdir -p "$malformed/missing/locus-$target"
printf '#!/bin/sh\nprintf "locus fixture\\n"\n' \
    > "$malformed/missing/locus-$target/locus"
tar -czf "$malformed/missing-member.tar.gz" \
    -C "$malformed/missing" "locus-$target"

# A valid archive that stores the locus member twice. tar -r appends a second
# locus entry; listing the member name twice on one command is not portable.
mkdir -p "$malformed/duplicate/locus-$target"
printf '#!/bin/sh\nprintf "locus fixture\\n"\n' \
    > "$malformed/duplicate/locus-$target/locus"
printf '#!/bin/sh\nprintf "mcp fixture\\n"\n' \
    > "$malformed/duplicate/locus-$target/locus-mcp"
tar -cf "$malformed/duplicate.tar" -C "$malformed/duplicate" "locus-$target"
tar -rf "$malformed/duplicate.tar" \
    -C "$malformed/duplicate" "locus-$target/locus"
gzip -c "$malformed/duplicate.tar" > "$malformed/duplicate-member.tar.gz"

# A valid archive whose locus member is a symlink, not a regular file.
mkdir -p "$malformed/link/locus-$target"
ln -s locus-mcp "$malformed/link/locus-$target/locus"
printf '#!/bin/sh\nprintf "mcp fixture\\n"\n' \
    > "$malformed/link/locus-$target/locus-mcp"
tar -czf "$malformed/link-member.tar.gz" -C "$malformed/link" "locus-$target"

# A valid archive whose locus member is an empty file.
mkdir -p "$malformed/empty/locus-$target"
: > "$malformed/empty/locus-$target/locus"
printf '#!/bin/sh\nprintf "mcp fixture\\n"\n' \
    > "$malformed/empty/locus-$target/locus-mcp"
tar -czf "$malformed/empty-binary.tar.gz" -C "$malformed/empty" "locus-$target"

malformed_archive_case() {
    local name=$1 archive_file=$2 message=$3
    local case_dir="$test_root/$name" status=0
    local dest="$case_dir/dest dir" tmp="$case_dir/tmp" old="$case_dir/old"
    mkdir -p "$dest" "$tmp" "$old"
    printf '#!/bin/sh\nprintf "old locus\\n"\n' > "$old/locus"
    printf '#!/bin/sh\nprintf "old mcp\\n"\n' > "$old/locus-mcp"
    cp "$old/locus" "$old/locus-mcp" "$dest/"
    chmod 755 "$dest/locus" "$dest/locus-mcp"
    local digest
    if command -v sha256sum >/dev/null 2>&1; then
        digest=$(sha256sum "$archive_file")
    else
        digest=$(shasum -a 256 "$archive_file")
    fi
    printf '%s  %s\n' "${digest%% *}" "$asset" > "$case_dir/sidecar"
    TEST_ROOT="$test_root" TEST_ASSET="$asset" \
        INSTALL_TEST_ARCHIVE="$archive_file" \
        INSTALL_TEST_SIDECAR="$case_dir/sidecar" \
        TEST_LOCKED="$case_dir/locked" \
        TEST_REAL_MKTEMP="$real_mktemp" TEST_REAL_MKDIR="$real_mkdir" \
        HOME="$test_root/home" TMPDIR="$tmp" PATH="$test_root/tools:$PATH" \
        bash "$root/scripts/install.sh" --version v1.2.3 --bin-dir "$dest" \
        > "$case_dir/out" 2> "$case_dir/err" || status=$?
    [[ $status -ne 0 ]] || fail "$name: installer unexpectedly succeeded"
    grep -Fq "locus installer: $message" "$case_dir/err" \
        || fail "$name: missing installer report: $(cat "$case_dir/err")"
    if grep -Fq 'Installed locus and locus-mcp' "$case_dir/out"; then
        fail "$name: reported success"
    fi
    cmp "$dest/locus" "$old/locus" || fail "$name: existing locus was replaced"
    cmp "$dest/locus-mcp" "$old/locus-mcp" \
        || fail "$name: existing locus-mcp was replaced"
    [[ -x $dest/locus && -x $dest/locus-mcp ]] \
        || fail "$name: existing executables lost their mode"
    [[ $(ls -A "$dest" | wc -l) -eq 2 ]] \
        || fail "$name: unexpected files in destination"
    [[ -z "$(ls -A "$tmp")" ]] || fail "$name: temporary files leaked"
    ok "$name"
}

malformed_archive_case 'malformed-not-gzip' "$malformed/not-gzip.tar.gz" \
    'archive must contain locus and locus-mcp'
malformed_archive_case 'malformed-truncated' "$malformed/truncated.tar.gz" \
    'archive must contain locus and locus-mcp'
malformed_archive_case 'malformed-missing-member' \
    "$malformed/missing-member.tar.gz" 'archive must contain locus and locus-mcp'
malformed_archive_case 'malformed-duplicate-member' \
    "$malformed/duplicate-member.tar.gz" \
    'archive must contain each binary exactly once'
malformed_archive_case 'malformed-link-member' \
    "$malformed/link-member.tar.gz" 'archive binaries must be regular files'
malformed_archive_case 'malformed-empty-binary' \
    "$malformed/empty-binary.tar.gz" 'archive contains an empty locus'

printf '%d installer tests passed; 0 failed.\n' "$passed"
