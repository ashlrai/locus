#!/usr/bin/env bash
# Local merge gate: compare the working tree (including untracked files) to origin/main.
set -euo pipefail
cd "$(dirname "$0")/.."
SECONDS=0
trap 'printf "Total: %ss\n" "$SECONDS"' EXIT
step() {
    local label=$1
    shift
    if "$@"; then
        printf 'PASS %s\n' "$label"
    else
        printf 'FAIL %s\n' "$label"
        exit 1
    fi
}
step fmt cargo fmt --all -- --check
changes=$(mktemp)
trap 'rm -f "$changes"; printf "Total: %ss\n" "$SECONDS"' EXIT
# Disable rename detection so both old and new crate paths are considered.
if ! git diff --no-renames --name-only -z origin/main -- > "$changes" ||
   ! git ls-files --others --exclude-standard -z >> "$changes"; then
    printf 'FAIL changed-crates\n'
    exit 1
fi
packages=()
all=false
while IFS= read -r -d '' path; do
    case "$path" in
        Cargo.toml|Cargo.lock|rust-toolchain*|.cargo/*) all=true ;;
    esac
    for manifest in crates/*/Cargo.toml; do
        crate=${manifest%/Cargo.toml}
        if [[ "$path" == "$crate/"* ]]; then
            name=${crate##*/}
            case " ${packages[*]-} " in
                *" $name "*) ;;
                *) packages+=(-p "$name") ;;
            esac
        fi
    done
done < "$changes"
if $all; then packages=(--workspace); fi
if ((${#packages[@]})); then
    step clippy cargo clippy --locked --all-targets "${packages[@]}" -- -D warnings
    step tests cargo test --locked "${packages[@]}"
else
    printf 'PASS clippy (no changed crates)\nPASS tests (no changed crates)\n'
fi
