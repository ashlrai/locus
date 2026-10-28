#!/usr/bin/env bash
# Fails if any tracked paths collide case-insensitively (they dirty worktrees on macOS/Windows disks).
set -euo pipefail
dups=$(git ls-files | awk '{print tolower($0)}' | sort | uniq -d)
if [ -n "$dups" ]; then
  echo "case-colliding tracked paths:" >&2
  while IFS= read -r d; do git ls-files | awk -v d="$d" 'tolower($0)==d' >&2; done <<<"$dups"
  exit 1
fi
echo "no case-colliding paths"
