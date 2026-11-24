#!/usr/bin/env bash
# One-command development setup for Linux and macOS.
# Installs Node 20 when it is missing, then runs scripts/setup.mjs, which
# installs Rust when it is missing, builds the CLI, and smokes `locus --version`.
# Safe to run again. Windows: powershell -File scripts/setup.ps1

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

if command -v node >/dev/null 2>&1; then
  exec node "$ROOT/scripts/setup.mjs"
fi

os="$(uname -s)"
case "$os" in
  Linux|Darwin) ;;
  *)
    printf 'setup.sh supports Linux and macOS (this host is %s). On Windows run: powershell -File scripts/setup.ps1\n' "$os" >&2
    exit 1
    ;;
esac

need() {
  if ! command -v "$1" >/dev/null 2>&1; then
    printf 'setup.sh needs %s on PATH\n' "$1" >&2
    exit 1
  fi
}

need curl
need tar

node_arch() {
  case "$(uname -m)" in
    x86_64|amd64) printf 'x64\n' ;;
    aarch64|arm64) printf 'arm64\n' ;;
    *)
      printf 'setup.sh: cannot install Node for architecture %s\n' "$(uname -m)" >&2
      return 1
      ;;
  esac
}

node_os() {
  case "$os" in
    Linux) printf 'linux\n' ;;
    Darwin) printf 'darwin\n' ;;
  esac
}

check_sha256() {
  local file="$1" hash="$2" dir
  dir="$(dirname "$file")"
  if command -v sha256sum >/dev/null 2>&1; then
    (cd "$dir" && printf '%s  %s\n' "$hash" "$(basename "$file")" | sha256sum -c -)
  elif command -v shasum >/dev/null 2>&1; then
    (cd "$dir" && printf '%s  %s\n' "$hash" "$(basename "$file")" | shasum -a 256 -c -)
  else
    printf 'setup.sh: need sha256sum or shasum to verify the Node tarball\n' >&2
    return 1
  fi
}

ensure_node() {
  local dest="${HOME}/.local/share/locus-setup/node"
  if [[ -x "${dest}/bin/node" ]]; then
    export PATH="${dest}/bin:${PATH}"
    return 0
  fi
  local arch osname sums name hash tmp extracted
  arch="$(node_arch)"
  osname="$(node_os)"
  printf 'setup.sh: installing Node 20 for %s-%s\n' "$osname" "$arch"
  sums="$(curl --proto '=https' --tlsv1.2 -fsSL https://nodejs.org/dist/latest-v20.x/SHASUMS256.txt)"
  name="$(printf '%s\n' "$sums" | awk -v os="$osname" -v arch="$arch" '$2 ~ "^node-v[0-9.]+-" os "-" arch "\\.tar\\.gz$" { print $2; exit }')"
  hash="$(printf '%s\n' "$sums" | awk -v n="$name" '$2 == n { print $1; exit }')"
  if [[ -z "$name" || -z "$hash" ]]; then
    printf 'setup.sh: Node 20 tarball for %s-%s was not listed\n' "$osname" "$arch" >&2
    exit 1
  fi
  tmp="$(mktemp -d)"
  curl --proto '=https' --tlsv1.2 -fsSL -o "${tmp}/${name}" "https://nodejs.org/dist/latest-v20.x/${name}"
  check_sha256 "${tmp}/${name}" "$hash"
  tar -xzf "${tmp}/${name}" -C "$tmp"
  extracted="${tmp}/${name%.tar.gz}"
  if [[ ! -x "${extracted}/bin/node" ]]; then
    printf 'setup.sh: Node tarball did not contain bin/node\n' >&2
    exit 1
  fi
  mkdir -p "$(dirname "$dest")"
  rm -rf "$dest"
  mv "$extracted" "$dest"
  rm -rf "$tmp"
  export PATH="${dest}/bin:${PATH}"
}

ensure_node
exec node "$ROOT/scripts/setup.mjs"
