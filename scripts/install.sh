#!/usr/bin/env bash
# Install both release binaries without touching credentials or shell profiles.
set -euo pipefail

die() { printf 'locus installer: %s\n' "$*" >&2; exit 1; }

usage() {
  cat <<'USAGE'
Usage: bash install.sh [--version VERSION] [--bin-dir DIRECTORY]

Install locus and locus-mcp from a verified GitHub release archive.
Defaults: latest release, $HOME/.local/bin. No sudo or Rust required.
VERSION accepts a release such as v0.5.1 or 0.5.1, or latest.
Supports macOS arm64/x86_64 and Linux x86_64 (glibc).
Requires curl, tar, and sha256sum or shasum. Never edits shell profiles.
USAGE
}

valid_version() {
  [[ $1 =~ ^v[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z][0-9A-Za-z.-]*)?$ ]]
}

main() {
  local version=latest bin_dir='' os arch target tool
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --version|--bin-dir)
        [[ $# -ge 2 && -n $2 && $2 != --* ]] || die "$1 requires a value"
        if [[ $1 == --version ]]; then version=$2; else bin_dir=$2; fi
        shift 2
        ;;
      -h|--help) usage; return ;;
      *) die "unknown option: $1 (see --help)" ;;
    esac
  done

  if [[ $version != latest ]]; then
    [[ $version == v* ]] || version="v$version"
    valid_version "$version" || die 'invalid release version'
  fi
  if [[ -z $bin_dir ]]; then
    [[ -n ${HOME:-} ]] || die 'HOME is unset; specify --bin-dir'
    bin_dir="$HOME/.local/bin"
  fi
  # Make relative paths safe even when they begin with a dash.
  [[ $bin_dir == /* ]] || bin_dir="$PWD/$bin_dir"

  for tool in curl tar uname mktemp mkdir chmod mv rm tr; do
    command -v "$tool" >/dev/null 2>&1 || die "required command not found: $tool"
  done
  local checksum_tool
  if command -v sha256sum >/dev/null 2>&1; then
    checksum_tool=sha256sum
  elif command -v shasum >/dev/null 2>&1; then
    checksum_tool=shasum
  else
    die 'checksum verification requires sha256sum or shasum'
  fi

  os=$(uname -s)
  arch=$(uname -m)
  case "$os/$arch" in
    Darwin/arm64|Darwin/aarch64) target=aarch64-apple-darwin ;;
    Darwin/x86_64|Darwin/amd64) target=x86_64-apple-darwin ;;
    Linux/x86_64|Linux/amd64) target=x86_64-unknown-linux-gnu ;;
    *) die "unsupported platform: $os/$arch; use a source install" ;;
  esac

  local releases=https://github.com/ashlrai/locus/releases resolved
  if [[ $version == latest ]]; then
    # Resolve once so a concurrent release cannot mix archive/sidecar versions.
    resolved=$(download --output /dev/null --write-out '%{url_effective}' "$releases/latest") \
      || die 'could not resolve the latest release'
    [[ $resolved == "$releases/tag/"* ]] || die 'unexpected latest release URL'
    version=${resolved#"$releases/tag/"}
    valid_version "$version" || die 'invalid latest release version'
  fi

  umask 077
  work_dir=$(mktemp -d "${TMPDIR:-/tmp}/locus-install.XXXXXX")
  local asset="locus-$target.tar.gz" root="locus-$target"
  local archive="$work_dir/locus-$target.tar.gz" sidecar="$work_dir/checksum"
  printf 'Downloading Locus %s (%s)...\n' "$version" "$target"
  download --output "$archive" "$releases/download/$version/$asset" \
    || die 'archive download failed; nothing installed'
  download --output "$sidecar" "$releases/download/$version/$asset.sha256" \
    || die 'checksum download failed; nothing installed'

  # Parse the expected filename ourselves: never let an untrusted checksum
  # sidecar choose another local path for a checksum utility to read.
  local hash name extra expected='' count=0
  while IFS=$' \t' read -r hash name extra || [[ -n $hash ]]; do
    count=$((count + 1))
    name=${name%$'\r'}
    [[ $hash =~ ^[0-9a-fA-F]{64}$ && ${name#\*} == "$asset" && -z $extra ]] \
      || die 'invalid checksum sidecar; nothing installed'
    expected=$hash
  done < "$sidecar"
  [[ $count -eq 1 ]] || die 'checksum sidecar must contain exactly one record'
  local actual
  if [[ $checksum_tool == sha256sum ]]; then
    actual=$(sha256sum "$archive")
  else
    actual=$(shasum -a 256 "$archive")
  fi
  actual=${actual%% *}
  expected=$(printf '%s' "$expected" | tr 'A-F' 'a-f')
  [[ $actual == "$expected" ]] || die 'SHA-256 checksum mismatch; nothing installed'

  # Only stream the two exact binary members to private files. Unrelated tar
  # entries (including traversal paths) are never extracted into the filesystem.
  # Check types and counts first: links, directories and duplicates are refused.
  unset TAR_OPTIONS
  tar -tvzf "$archive" "$root/locus" "$root/locus-mcp" > "$work_dir/members" \
    || die 'archive must contain locus and locus-mcp'
  local member
  count=0
  while IFS= read -r member; do
    [[ $member == -* ]] || die 'archive binaries must be regular files'
    count=$((count + 1))
  done < "$work_dir/members"
  [[ $count -eq 2 ]] || die 'archive must contain each binary exactly once'
  local binary
  for binary in locus locus-mcp; do
    tar -xOzf "$archive" "$root/$binary" > "$work_dir/$binary" \
      || die "could not unpack $binary"
    [[ -s $work_dir/$binary ]] || die "archive contains an empty $binary"
    [[ ! -L $bin_dir/$binary && ! -d $bin_dir/$binary ]] \
      || die "refusing symlink or directory destination: $bin_dir/$binary"
  done

  mkdir -p -- "$bin_dir" || die 'cannot create install directory; choose a writable --bin-dir'
  # Stage on the destination filesystem before any replacement. Each rename
  # replaces the old inode, so an upgrade also works with a running binary.
  stage_dir=$(mktemp -d "$bin_dir/.locus-install.XXXXXX")
  for binary in locus locus-mcp; do
    mv -- "$work_dir/$binary" "$stage_dir/$binary"
    chmod 755 "$stage_dir/$binary"
  done
  for binary in locus locus-mcp; do
    mv -f -- "$stage_dir/$binary" "$bin_dir/$binary"
  done
  printf 'Installed locus and locus-mcp %s in %s\n' "$version" "$bin_dir"
  case ":${PATH:-}:" in
    *":$bin_dir:"*) ;;
    *) printf 'Add this directory to your PATH: %s\n' "$bin_dir" ;;
  esac
}

download() {
  # Ignore ~/.curlrc and forbid HTTP on initial requests and redirects.
  curl -q --fail --silent --show-error --location \
    --proto '=https' --proto-redir '=https' --tlsv1.2 \
    --connect-timeout 15 --max-time 300 --retry 2 "$@"
}

work_dir=''
stage_dir=''
cleanup() {
  [[ -z $work_dir ]] || rm -rf -- "$work_dir"
  [[ -z $stage_dir ]] || rm -rf -- "$stage_dir"
  return 0
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
main "$@"
