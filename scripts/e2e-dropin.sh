# Sourced by scripts/e2e.sh (and scripts/check-e2e-dropin.sh); not executable.
# Runs every <dir>/*.sh in sorted order by sourcing it into the caller's shell,
# so a step can use log/ok/skip/die, locus, and the e2e environment. A step
# that fails (die, `exit`, or a failing command under set -e) fails the run.
#
# Usage: e2e_run_dropins [dir]   (default: E2E_DROPIN_DIR, else scripts/e2e.d)
e2e_run_dropins() {
  local dir="${1:-${E2E_DROPIN_DIR:-$ROOT/scripts/e2e.d}}" step
  local LC_ALL=C
  [[ -d "$dir" ]] || return 0
  [[ -r "$dir" && -x "$dir" ]] || { die "cannot enumerate e2e drop-ins"; return 1; }
  # Bash pathname expansion is sorted and preserves spaces/newlines. Avoid a
  # find|sort process substitution: its failure cannot reach the caller.
  for step in "$dir"/*.sh; do
    # Also handles an unmatched glob; match find -type f by ignoring symlinks.
    [[ -f "$step" && ! -L "$step" ]] || continue
    log "drop-in ${step##*/}"
    # shellcheck disable=SC1090
    source "$step"
  done
}
