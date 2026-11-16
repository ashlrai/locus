#!/usr/bin/env bash
# Exercise archive selection through the installer without network access.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
test_root=$(mktemp -d "${TMPDIR:-/tmp}/locus-archive-test.XXXXXX")
trap 'rm -rf -- "$test_root"' EXIT
mkdir -p "$test_root/tools" "$test_root/fixtures" "$test_root/home"

for target in aarch64-apple-darwin x86_64-apple-darwin x86_64-unknown-linux-gnu; do
    asset="locus-$target.tar.gz"
    package="$test_root/fixtures/locus-$target"
    mkdir -p "$package"
    for binary in locus locus-mcp; do
        printf '%s fixture for %s\n' "$binary" "$target" > "$package/$binary"
    done
    tar -czf "$test_root/fixtures/$asset" -C "$test_root/fixtures" "locus-$target"
    if command -v sha256sum >/dev/null 2>&1; then
        digest=$(sha256sum "$test_root/fixtures/$asset")
    else
        digest=$(shasum -a 256 "$test_root/fixtures/$asset")
    fi
    printf '%s  %s\n' "${digest%% *}" "$asset" > "$test_root/fixtures/$asset.sha256"
done

cat > "$test_root/tools/uname" <<'MOCK'
#!/usr/bin/env bash
case "$1" in
    -s) printf '%s\n' "$FIXTURE_OS" ;;
    -m) printf '%s\n' "$FIXTURE_ARCH" ;;
    *) exit 1 ;;
esac
MOCK
cat > "$test_root/tools/curl" <<'MOCK'
#!/usr/bin/env bash
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
printf '%s\n' "$url" >> "$FIXTURE_REQUESTS"
case "$url" in
    "https://github.com/ashlrai/locus/releases/download/v1.2.3/"*)
        cp "$FIXTURE_DIR/${url##*/}" "$out" ;;
    *) exit 1 ;;
esac
MOCK
chmod +x "$test_root/tools/uname" "$test_root/tools/curl"

passed=0
check_platform() {
    local os=$1 arch=$2 target=$3 case_dir="$test_root/$1-$2"
    local asset="locus-$3.tar.gz" base='https://github.com/ashlrai/locus/releases/download/v1.2.3'
    mkdir -p "$case_dir"
    FIXTURE_OS="$os" FIXTURE_ARCH="$arch" FIXTURE_DIR="$test_root/fixtures" \
        FIXTURE_REQUESTS="$case_dir/requests" HOME="$test_root/home" \
        TMPDIR="$case_dir" PATH="$test_root/tools:$PATH" \
        bash "$root/scripts/install.sh" --version v1.2.3 --bin-dir "$case_dir/bin" \
        > "$case_dir/output"
    printf '%s/%s\n%s/%s.sha256\n' "$base" "$asset" "$base" "$asset" > "$case_dir/expected"
    cmp "$case_dir/expected" "$case_dir/requests" || { printf 'Wrong requests for %s/%s\n' "$os" "$arch" >&2; exit 1; }
    for binary in locus locus-mcp; do
        cmp "$test_root/fixtures/locus-$target/$binary" "$case_dir/bin/$binary"
        [[ -x $case_dir/bin/$binary ]]
    done
    passed=$((passed + 1))
    printf 'ok %d - %s/%s selects %s\n' "$passed" "$os" "$arch" "$target"
}

check_platform Darwin arm64 aarch64-apple-darwin
check_platform Darwin aarch64 aarch64-apple-darwin
check_platform Darwin x86_64 x86_64-apple-darwin
check_platform Darwin amd64 x86_64-apple-darwin
check_platform Linux x86_64 x86_64-unknown-linux-gnu
check_platform Linux amd64 x86_64-unknown-linux-gnu
printf '%d archive selection tests passed; 0 failed.\n' "$passed"
