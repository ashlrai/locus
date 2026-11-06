#!/usr/bin/env bash
# check-support-links.sh -- verify every relative link in SUPPORT.md resolves on disk.
#
#   bash scripts/check-support-links.sh [path/to/SUPPORT.md]
#
# Default file: SUPPORT.md at the repository root. Relative destinations resolve
# next to the file (GitHub semantics); /-prefixed destinations resolve from the
# repository root. Checked: inline links and images, and reference definitions
# (`[id]: target`). Skipped: fenced code, external schemes (https:, mailto:, ...),
# protocol-relative URLs, and same-file #anchors. Anchor fragments on relative
# links are dropped, so only the file part must exist; scripts/check-readme-links.mjs
# checks anchors.
#
# Exit 0 when every relative link resolves. Exit 1 prints one line per broken
# link. Exit 2 when the file cannot be read.
#
# Bash 3.2 compatible (macOS default shell): no associative arrays or mapfile.
set -euo pipefail

repository=$(cd "$(dirname "$0")/.." && pwd)
file=${1:-$repository/SUPPORT.md}

if [ ! -r "$file" ]; then
  printf 'check-support-links: cannot read %s\n' "$file" >&2
  exit 2
fi

base=$(cd "$(dirname "$file")" && pwd)

# Emit "<line>\t<raw destination>" for every link candidate outside code fences.
extract() {
  awk '
    /^[[:space:]]*(```|~~~)/ { fence = !fence; next }
    fence { next }
    {
      line = $0
      while (match(line, /\]\([^)]*\)/)) {
        dest = substr(line, RSTART + 2, RLENGTH - 3)
        line = substr(line, RSTART + RLENGTH)
        print NR "\t" dest
      }
      if (match($0, /^[[:space:]]*\[[^]]+\]:[[:space:]]*[^[:space:]]+/)) {
        def = substr($0, RSTART, RLENGTH)
        sub(/^[[:space:]]*\[[^]]+\]:[[:space:]]*/, "", def)
        print NR "\t" def
      }
    }
  ' "$1"
}

broken=0
checked=0
while IFS=$'\t' read -r line raw; do
  # Destination is the first word, or the <...> span (which may hold spaces); a "title" follows it.
  if [[ $raw == \<* ]]; then
    target=${raw#<}
    target=${target%%>*}
  else
    target=${raw%% *}
  fi
  case "$target" in
    '' | '#'* | //*) continue ;;
  esac
  # External scheme (https:, mailto:, ...). A colon after a path separator is a filename.
  if [[ $target =~ ^[a-zA-Z][a-zA-Z0-9+.-]*: ]]; then continue; fi
  path=${target%%#*}
  path=${path%%\?*}
  [ -n "$path" ] || continue
  checked=$((checked + 1))
  case "$path" in
    /*) resolved=$repository$path ;;
    *) resolved=$base/$path ;;
  esac
  if [ ! -e "$resolved" ]; then
    printf 'broken link in %s:%s: %s\n' "$file" "$line" "$target" >&2
    broken=$((broken + 1))
  fi
done < <(extract "$file")

if [ "$broken" -gt 0 ]; then
  printf 'check-support-links: %d broken of %d relative links\n' "$broken" "$checked" >&2
  exit 1
fi
printf 'check-support-links: ok (%d relative links in %s)\n' "$checked" "$file"
