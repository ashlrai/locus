#!/usr/bin/env bash
# Offline regression for an installation directory containing spaces.
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

destination="$test_root/install directory/nested bin"
TEST_ROOT="$test_root" TEST_ASSET="$asset" HOME="$test_root/home" \
    PATH="$test_root/tools:$PATH" bash "$root/scripts/install.sh" \
    --version v1.2.3 --bin-dir "$destination" > "$test_root/output"

for binary in locus locus-mcp; do
    [[ -x "$destination/$binary" ]] || { printf 'missing executable: %s\n' "$binary" >&2; exit 1; }
    cmp "$destination/$binary" "$test_root/fixture/locus-$target/$binary"
done
[[ $("$destination/locus") == 'locus fixture' ]]
[[ $("$destination/locus-mcp") == 'mcp fixture' ]]
[[ ! -e "$test_root/install" ]]
printf '1 installer spaces test passed; 0 failed.\n'
