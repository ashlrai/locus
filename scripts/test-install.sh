#!/usr/bin/env bash
# Offline installer regression tests. No release downloads or real HOME writes.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
INSTALLER="$ROOT/scripts/install.sh"
BASH_BIN=$(command -v bash)
REAL_TAR=$(command -v tar)
if command -v sha256sum >/dev/null 2>&1; then
    HASH_BIN=$(command -v sha256sum)
else
    HASH_BIN=$(command -v shasum)
fi
TEST_ROOT=$(mktemp -d "${TMPDIR:-/tmp}/locus-install-test.XXXXXX")
trap 'rm -rf "$TEST_ROOT"' EXIT
passed=0

# A deliberately restricted PATH makes the absence of either hash tool real,
# even on machines where another copy is installed in a system directory.
mkdir "$TEST_ROOT/tools"
# GNU tar invokes gzip from PATH; BSD tar handles gzip internally.
for tool in cat chmod cp gzip mkdir mktemp mv rm tr; do
    if command -v "$tool" >/dev/null 2>&1; then
        ln -s "$(command -v "$tool")" "$TEST_ROOT/tools/$tool"
    fi
done
cat > "$TEST_ROOT/tools/uname" <<'MOCK'
#!/bin/bash
case "$1" in
    -s) printf '%s\n' "$TEST_OS" ;;
    -m) printf '%s\n' "$TEST_ARCH" ;;
    *) exit 2 ;;
esac
MOCK
cat > "$TEST_ROOT/tools/curl" <<'MOCK'
#!/bin/bash
set -euo pipefail
[[ "$1" == -q ]] || { echo 'curl must disable curlrc' >&2; exit 2; }
shift
out= write_out= url= proto= redir= tls= fail= location=
while (($#)); do
    case "$1" in
        --output|-o) out=$2; shift 2 ;;
        --write-out|-w) write_out=$2; shift 2 ;;
        --proto) proto=$2; shift 2 ;;
        --proto-redir) redir=$2; shift 2 ;;
        --tlsv1.2) tls=yes; shift ;;
        --fail|-f) fail=yes; shift ;;
        --location|-L) location=yes; shift ;;
        --silent|--show-error|-s|-S|-sS) shift ;;
        --retry|--connect-timeout|--max-time) shift 2 ;;
        https://*) url=$1; shift ;;
        *) echo 'unexpected curl argument' >&2; exit 2 ;;
    esac
done
[[ "$proto" == '=https' && "$redir" == '=https' && "$tls" == yes && "$fail" == yes && "$location" == yes ]] || {
    echo 'curl must enforce HTTPS, TLS, and HTTP failures' >&2; exit 2;
}
printf '%s\n' "$url" >> "$TEST_CASE/downloads"
case "$url" in
    https://github.com/ashlrai/locus/releases/latest)
        [[ "${TEST_FAIL_DOWNLOAD:-}" != latest ]] || exit 22
        [[ "$out" == /dev/null && "$write_out" == '%{url_effective}' ]] || exit 2
        printf '%s' "$TEST_LATEST_URL"
        ;;
    "https://github.com/ashlrai/locus/releases/download/$TEST_VERSION/locus-$TEST_TARGET.tar.gz.sha256")
        [[ "${TEST_FAIL_DOWNLOAD:-}" != checksum ]] || exit 22
        cp "$TEST_CHECKSUM" "$out"
        ;;
    "https://github.com/ashlrai/locus/releases/download/$TEST_VERSION/locus-$TEST_TARGET.tar.gz")
        [[ "${TEST_FAIL_DOWNLOAD:-}" != archive ]] || exit 22
        cp "$TEST_ARCHIVE" "$out"
        ;;
    *) echo 'unexpected release URL' >&2; exit 22 ;;
esac
MOCK
cat > "$TEST_ROOT/tools/tar" <<'MOCK'
#!/bin/bash
printf '%s\n' called >> "$TEST_CASE/tar-calls"
if [[ "$1" == -xOzf && "${3##*/}" == "${TEST_FAIL_UNPACK:-}" ]]; then
    echo 'simulated extraction failure' >&2
    exit 1
fi
exec "$TEST_REAL_TAR" "$@"
MOCK
cat > "$TEST_ROOT/hash" <<'MOCK'
#!/bin/bash
set -euo pipefail
printf '%s\n' "${0##*/}" >> "$TEST_CASE/hash-calls"
if [[ "${0##*/}" == shasum ]]; then
    [[ "$1" == -a && "$2" == 256 ]] || exit 2
    shift 2
fi
if [[ "$TEST_REAL_HASH_KIND" == shasum ]]; then
    exec "$TEST_REAL_HASH" -a 256 "$@"
else
    exec "$TEST_REAL_HASH" "$@"
fi
MOCK
chmod +x "$TEST_ROOT/tools/"{uname,curl,tar} "$TEST_ROOT/hash"

new_case() {
    TEST_CASE="$TEST_ROOT/case-$passed"
    mkdir -p "$TEST_CASE/home" "$TEST_CASE/tmp" "$TEST_CASE/bin" "$TEST_CASE/stage"
    cp -R "$TEST_ROOT/tools/." "$TEST_CASE/bin/"
    ln -s "$TEST_ROOT/hash" "$TEST_CASE/bin/sha256sum"
    ln -s "$TEST_ROOT/hash" "$TEST_CASE/bin/shasum"
    TEST_OS=Linux
    TEST_ARCH=x86_64
    TEST_TARGET=x86_64-unknown-linux-gnu
    TEST_VERSION=v1.2.3
    TEST_LATEST_URL=https://github.com/ashlrai/locus/releases/tag/v1.2.3
    TEST_FAIL_DOWNLOAD=
    TEST_FAIL_UNPACK=
    TEST_ARCHIVE="$TEST_CASE/release.tar.gz"
    TEST_CHECKSUM="$TEST_CASE/release.sha256"
    TEST_REAL_TAR=$REAL_TAR
    TEST_REAL_HASH=$HASH_BIN
    TEST_REAL_HASH_KIND=${HASH_BIN##*/}
    export TEST_CASE TEST_OS TEST_ARCH TEST_TARGET TEST_VERSION TEST_LATEST_URL
    export TEST_FAIL_DOWNLOAD TEST_FAIL_UNPACK TEST_ARCHIVE TEST_CHECKSUM TEST_REAL_TAR TEST_REAL_HASH TEST_REAL_HASH_KIND
}

fixture() {
    local dir="$TEST_CASE/stage/locus-$TEST_TARGET"
    mkdir -p "$dir"
    printf '#!/bin/sh\nprintf "locus fixture\\n"\n' > "$dir/locus"
    printf '#!/bin/sh\nprintf "mcp fixture\\n"\n' > "$dir/locus-mcp"
    "$REAL_TAR" -czf "$TEST_ARCHIVE" -C "$TEST_CASE/stage" "locus-$TEST_TARGET"
    checksum
}

checksum() {
    local digest
    if [[ "${HASH_BIN##*/}" == shasum ]]; then
        digest=$("$HASH_BIN" -a 256 "$TEST_ARCHIVE")
    else
        digest=$("$HASH_BIN" "$TEST_ARCHIVE")
    fi
    digest=${digest%% *}
    printf '%s  locus-%s.tar.gz\n' "$digest" "$TEST_TARGET" > "$TEST_CHECKSUM"
}

run_installer() {
    HOME="$TEST_CASE/home" TMPDIR="$TEST_CASE/tmp" PATH="$TEST_CASE/bin" \
        "$BASH_BIN" "$INSTALLER" "$@" > "$TEST_CASE/stdout" 2> "$TEST_CASE/stderr"
}

fail() {
    printf 'FAIL: %s\n' "$1" >&2
    exit 1
}

ok() {
    passed=$((passed + 1))
    printf 'ok %d - %s\n' "$passed" "$1"
}

assert_clean() {
    [[ -z "$(ls -A "$TEST_CASE/tmp")" ]] || fail 'temporary files leaked'
}

success() {
    run_installer "$@" || fail 'installer unexpectedly failed'
    assert_clean
}

rejection() {
    if run_installer "$@"; then fail 'installer unexpectedly succeeded'; fi
    assert_clean
}

assert_binaries() {
    local dest=$1
    [[ -x "$dest/locus" && -x "$dest/locus-mcp" ]] || fail 'missing executable binaries'
    cmp "$dest/locus" "$TEST_CASE/stage/locus-$TEST_TARGET/locus" || fail 'wrong locus bytes'
    cmp "$dest/locus-mcp" "$TEST_CASE/stage/locus-$TEST_TARGET/locus-mcp" || fail 'wrong MCP bytes'
}

seed_install() {
    mkdir -p "$TEST_CASE/home/.local/bin"
    printf 'old locus\n' > "$TEST_CASE/home/.local/bin/locus"
    printf 'old mcp\n' > "$TEST_CASE/home/.local/bin/locus-mcp"
}

assert_preserved() {
    [[ "$(cat "$TEST_CASE/home/.local/bin/locus")" == 'old locus' ]] || fail 'old locus changed'
    [[ "$(cat "$TEST_CASE/home/.local/bin/locus-mcp")" == 'old mcp' ]] || fail 'old MCP changed'
}

for platform in linux-x86_64 linux-amd64 darwin-arm64 darwin-aarch64 darwin-x86_64 darwin-amd64; do
    new_case
    case "$platform" in
        linux-x86_64) ;;
        linux-amd64) TEST_ARCH=amd64 ;;
        darwin-arm64) TEST_OS=Darwin; TEST_ARCH=arm64; TEST_TARGET=aarch64-apple-darwin ;;
        darwin-aarch64) TEST_OS=Darwin; TEST_ARCH=aarch64; TEST_TARGET=aarch64-apple-darwin ;;
        darwin-x86_64) TEST_OS=Darwin; TEST_TARGET=x86_64-apple-darwin ;;
        darwin-amd64) TEST_OS=Darwin; TEST_ARCH=amd64; TEST_TARGET=x86_64-apple-darwin ;;
    esac
    fixture
    success --version v1.2.3
    assert_binaries "$TEST_CASE/home/.local/bin"
    grep -q '^sha256sum$' "$TEST_CASE/hash-calls" || fail 'sha256sum was not preferred'
    ok "installs both binaries for $platform"
done

new_case; fixture
success
assert_binaries "$TEST_CASE/home/.local/bin"
grep -q '/releases/latest$' "$TEST_CASE/downloads" || fail 'latest release was not resolved'
[[ $(wc -l < "$TEST_CASE/downloads") -eq 3 ]] || fail 'latest was resolved more than once'
grep -q 'PATH' "$TEST_CASE/stdout" || fail 'missing PATH guidance'
ok 'latest resolves once to an immutable release tag'

new_case; fixture; seed_install
success --version v1.2.3
assert_binaries "$TEST_CASE/home/.local/bin"
success --version v1.2.3
assert_binaries "$TEST_CASE/home/.local/bin"
[[ -z "$(find "$TEST_CASE/home/.local/bin" -name '.locus-install.*' -print)" ]] || fail 'destination staging files leaked'
ok 'upgrades existing binaries and safely repeats installation'

new_case; fixture
TAR_OPTIONS=--definitely-invalid-locus-option run_installer --version v1.2.3 || fail 'ambient TAR_OPTIONS changed extraction'
assert_binaries "$TEST_CASE/home/.local/bin"
assert_clean
ok 'ignores ambient TAR_OPTIONS'

new_case; fixture
success --version 1.2.3 --bin-dir "$TEST_CASE/custom bin"
assert_binaries "$TEST_CASE/custom bin"
[[ ! -e "$TEST_CASE/home/.local/bin" ]] || fail 'custom destination ignored'
[[ ! -e "$TEST_CASE/home/.bashrc" && ! -e "$TEST_CASE/home/.zshrc" ]] || fail 'shell startup files modified'
ok 'normalizes version and supports destination with spaces'

new_case; fixture
HOME='' TMPDIR="$TEST_CASE/tmp" PATH="$TEST_CASE/bin" \
    "$BASH_BIN" "$INSTALLER" --version v1.2.3 > "$TEST_CASE/stdout" 2> "$TEST_CASE/stderr" && fail 'unset HOME accepted without destination'
grep -q 'HOME is unset' "$TEST_CASE/stderr" || fail 'missing HOME guidance'
[[ ! -e "$TEST_CASE/downloads" ]] || fail 'missing HOME triggered a download'
HOME='' TMPDIR="$TEST_CASE/tmp" PATH="$TEST_CASE/bin" \
    "$BASH_BIN" "$INSTALLER" --version v1.2.3 --bin-dir "$TEST_CASE/custom" > "$TEST_CASE/stdout" 2> "$TEST_CASE/stderr" || fail 'explicit destination required HOME'
assert_binaries "$TEST_CASE/custom"
assert_clean
ok 'requires HOME only for the default destination'

new_case; fixture
HOME="$TEST_CASE/home" TMPDIR="$TEST_CASE/tmp" PATH="$TEST_CASE/bin:$TEST_CASE/home/.local/bin" \
    "$BASH_BIN" "$INSTALLER" --version v1.2.3 > "$TEST_CASE/stdout" 2> "$TEST_CASE/stderr" || fail 'installation with destination on PATH failed'
assert_binaries "$TEST_CASE/home/.local/bin"
assert_clean
if grep -q 'Add this directory to your PATH' "$TEST_CASE/stdout"; then fail 'unnecessary PATH guidance'; fi
ok 'omits PATH guidance when destination is already on PATH'

new_case; fixture
rm "$TEST_CASE/bin/sha256sum"
success --version v1.2.3
grep -q '^shasum$' "$TEST_CASE/hash-calls" || fail 'shasum fallback not used'
ok 'uses shasum -a 256 fallback'

new_case; fixture
rm "$TEST_CASE/bin/sha256sum" "$TEST_CASE/bin/shasum"
rejection --version v1.2.3
[[ ! -e "$TEST_CASE/tar-calls" ]] || fail 'archive opened without verifier'
ok 'rejects missing checksum verifiers'

for required in curl tar; do
    new_case
    rm "$TEST_CASE/bin/$required"
    rejection --version v1.2.3
    [[ ! -e "$TEST_CASE/downloads" ]] || fail 'missing required tool was detected after download'
    ok "rejects missing $required before network"
done

for args in help unknown missing-version missing-destination bad-version; do
    new_case
    case "$args" in
        help) success --help ;;
        unknown) rejection --unexpected ;;
        missing-version) rejection --version ;;
        missing-destination) rejection --bin-dir ;;
        bad-version) rejection --version ../../bad ;;
    esac
    [[ ! -e "$TEST_CASE/downloads" ]] || fail 'option parsing unexpectedly downloaded'
    ok "handles $args without network"
done

for platform in Linux-arm64 Linux-aarch64 Windows-x86_64 FreeBSD-x86_64 Darwin-ppc; do
    new_case; seed_install
    TEST_OS=${platform%-*}; TEST_ARCH=${platform##*-}
    rejection --version v1.2.3
    [[ ! -e "$TEST_CASE/downloads" ]] || fail 'unsupported platform downloaded'
    assert_preserved
    grep -Fq "unsupported platform: $TEST_OS/$TEST_ARCH; use a source install" "$TEST_CASE/stderr" || fail 'missing unsupported platform error'
    ok "rejects unsupported $platform"
done

for download in archive checksum latest; do
    new_case; fixture; seed_install
    TEST_FAIL_DOWNLOAD=$download
    rejection
    assert_preserved
    [[ ! -e "$TEST_CASE/tar-calls" ]] || fail 'failed download opened archive'
    case "$download" in
        latest) message='could not resolve the latest release' ;;
        archive) message='archive download failed; nothing installed' ;;
        checksum) message='checksum download failed; nothing installed' ;;
    esac
    grep -Fq "$message" "$TEST_CASE/stderr" || fail "missing $download error message"
    ok "preserves existing installation on $download download failure"
done

for latest in 'https://evil.invalid/releases/tag/v1.2.3' 'https://github.com/ashlrai/locus/releases/latest' 'https://github.com/ashlrai/locus/releases/tag/../../bad'; do
    new_case; fixture
    TEST_LATEST_URL=$latest
    rejection
    [[ ! -e "$TEST_CASE/tar-calls" ]] || fail 'invalid latest tag opened archive'
    ok 'rejects invalid latest redirect'
done

for sidecar in empty malformed wrong-name duplicate mismatch; do
    new_case; fixture; seed_install
    case "$sidecar" in
        empty) : > "$TEST_CHECKSUM" ;;
        malformed) printf 'not a digest\n' > "$TEST_CHECKSUM" ;;
        wrong-name) sed 's/\.tar.gz/.zip/' "$TEST_CHECKSUM" > "$TEST_CHECKSUM.new"; mv "$TEST_CHECKSUM.new" "$TEST_CHECKSUM" ;;
        duplicate) cat "$TEST_CHECKSUM" "$TEST_CHECKSUM" > "$TEST_CHECKSUM.new"; mv "$TEST_CHECKSUM.new" "$TEST_CHECKSUM" ;;
        mismatch) printf '%064d  locus-%s.tar.gz\n' 0 "$TEST_TARGET" > "$TEST_CHECKSUM" ;;
    esac
    rejection --version v1.2.3
    assert_preserved
    [[ ! -e "$TEST_CASE/tar-calls" ]] || fail 'archive opened before checksum validation'
    case "$sidecar" in
        empty|duplicate) message='checksum sidecar must contain exactly one record' ;;
        malformed|wrong-name) message='invalid checksum sidecar; nothing installed' ;;
        mismatch) message='SHA-256 checksum mismatch; nothing installed' ;;
    esac
    grep -Fq "$message" "$TEST_CASE/stderr" || fail "missing $sidecar checksum error"
    ok "rejects $sidecar checksum before tar"
done

new_case; fixture
sed 's/  / */' "$TEST_CHECKSUM" > "$TEST_CHECKSUM.new"; mv "$TEST_CHECKSUM.new" "$TEST_CHECKSUM"
success --version v1.2.3
ok 'accepts standard binary-mode checksum marker'

for broken in missing empty symlink hardlink directory duplicate corrupt; do
    new_case; fixture; seed_install
    dir="$TEST_CASE/stage/locus-$TEST_TARGET"
    case "$broken" in
        missing) rm "$dir/locus-mcp" ;;
        empty) : > "$dir/locus-mcp" ;;
        symlink) rm "$dir/locus-mcp"; ln -s locus "$dir/locus-mcp" ;;
        hardlink) rm "$dir/locus-mcp"; ln "$dir/locus" "$dir/locus-mcp" ;;
        directory) rm "$dir/locus-mcp"; mkdir "$dir/locus-mcp" ;;
        duplicate) ;;
        corrupt) ;;
    esac
    if [[ "$broken" == duplicate ]]; then
        "$REAL_TAR" -czf "$TEST_ARCHIVE" -C "$TEST_CASE/stage" "locus-$TEST_TARGET" "locus-$TEST_TARGET/locus"
    elif [[ "$broken" == corrupt ]]; then
        printf 'not a tar archive\n' > "$TEST_ARCHIVE"
    else
        "$REAL_TAR" -czf "$TEST_ARCHIVE" -C "$TEST_CASE/stage" "locus-$TEST_TARGET"
    fi
    checksum
    rejection --version v1.2.3
    assert_preserved
    ok "rejects $broken binary archive before replacing installation"
done

for binary in locus locus-mcp; do
    new_case; fixture; seed_install
    TEST_FAIL_UNPACK=$binary
    rejection --version v1.2.3
    assert_preserved
    grep -Fq "could not unpack $binary" "$TEST_CASE/stderr" || fail "missing $binary extraction error"
    if grep -Fq 'Installed locus and locus-mcp' "$TEST_CASE/stdout"; then fail 'reported success after extraction failure'; fi
    ok "preserves existing installation when extracting $binary fails"
done

new_case; fixture
printf 'archive must not write this file\n' > "$TEST_CASE/absolute-victim"
"$REAL_TAR" -czPf "$TEST_ARCHIVE" -C "$TEST_CASE/stage" "locus-$TEST_TARGET" "$TEST_CASE/absolute-victim"
checksum
rm "$TEST_CASE/absolute-victim"
success --version v1.2.3
assert_binaries "$TEST_CASE/home/.local/bin"
[[ ! -e "$TEST_CASE/absolute-victim" ]] || fail 'unrelated absolute archive member was extracted'
ok 'streams only binaries without extracting unrelated absolute paths'

new_case; fixture
printf 'archive must not write this file\n' > "$TEST_CASE/traversal-victim"
"$REAL_TAR" -czPf "$TEST_ARCHIVE" -C "$TEST_CASE/stage" "locus-$TEST_TARGET" ../traversal-victim
checksum
rm "$TEST_CASE/traversal-victim"
(cd "$TEST_CASE/stage" && success --version v1.2.3)
assert_binaries "$TEST_CASE/home/.local/bin"
[[ ! -e "$TEST_CASE/traversal-victim" ]] || fail 'unrelated traversal archive member was extracted'
ok 'streams only binaries without extracting unrelated traversal paths'

for destination in symlink directory; do
    new_case; fixture; seed_install
    rm "$TEST_CASE/home/.local/bin/locus-mcp"
    if [[ "$destination" == symlink ]]; then
        printf 'outside\n' > "$TEST_CASE/outside"
        ln -s "$TEST_CASE/outside" "$TEST_CASE/home/.local/bin/locus-mcp"
    else
        mkdir "$TEST_CASE/home/.local/bin/locus-mcp"
    fi
    rejection --version v1.2.3
    [[ "$(cat "$TEST_CASE/home/.local/bin/locus")" == 'old locus' ]] || fail 'first binary replaced before destination validation'
    if [[ "$destination" == symlink ]]; then
        [[ "$(cat "$TEST_CASE/outside")" == outside ]] || fail 'symlink target overwritten'
    fi
    ok "rejects existing $destination destination without partial upgrade"
done

new_case; fixture
printf 'existing file\n' > "$TEST_CASE/not-a-directory"
rejection --version v1.2.3 --bin-dir "$TEST_CASE/not-a-directory"
[[ "$(cat "$TEST_CASE/not-a-directory")" == 'existing file' ]] || fail 'non-directory destination changed'
ok 'rejects install directory that is an existing file'

new_case; fixture; seed_install
REAL_MV=$(command -v mv)
export REAL_MV
rm "$TEST_CASE/bin/mv"
cat > "$TEST_CASE/bin/mv" <<'MOCK'
#!/bin/bash
for arg in "$@"; do
    if [[ "$arg" == "$TEST_CASE/home/.local/bin/locus-mcp" ]]; then
        echo 'simulated final rename failure' >&2
        exit 1
    fi
done
exec "$REAL_MV" "$@"
MOCK
chmod +x "$TEST_CASE/bin/mv"
rejection --version v1.2.3
cmp "$TEST_CASE/home/.local/bin/locus" "$TEST_CASE/stage/locus-$TEST_TARGET/locus" || fail 'first binary was not replaced'
[[ "$(cat "$TEST_CASE/home/.local/bin/locus-mcp")" == 'old mcp' ]] || fail 'second binary changed despite rename failure'
grep -Fq 'simulated final rename failure' "$TEST_CASE/stderr" || fail 'missing rename error'
if grep -Fq 'Installed locus and locus-mcp' "$TEST_CASE/stdout"; then fail 'reported success after rename failure'; fi
[[ -z "$(find "$TEST_CASE/home/.local/bin" -name '.locus-install.*' -print)" ]] || fail 'destination staging files leaked'
ok 'reports second rename failure and leaves a partial upgrade'

printf '%d installer tests passed; 0 failed.\n' "$passed"
