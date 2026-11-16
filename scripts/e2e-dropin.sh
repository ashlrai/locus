# Sourced by scripts/e2e.sh (and scripts/check-e2e-dropin.sh); not executable.
# Runs every <dir>/*.sh in sorted order by sourcing it into the caller's shell,
# so a step can use log/ok/skip/die, locus, and the e2e environment. A step
# that fails (die, `exit`, or a failing command under set -e) fails the run.
#
# Usage: e2e_run_dropins [dir]   (default: E2E_DROPIN_DIR, else scripts/e2e.d)
e2e_run_dropins() {
  local dir="${1:-${E2E_DROPIN_DIR:-$ROOT/scripts/e2e.d}}" step
  local -a steps=()
  [[ -d "$dir" ]] || return 0
  # LC_ALL=C pins the order across locales.
  while IFS= read -r step; do
    steps+=("$step")
  done < <(LC_ALL=C find "$dir" -maxdepth 1 -type f -name '*.sh' | LC_ALL=C sort)
  for step in ${steps[@]+"${steps[@]}"}; do
    log "drop-in ${step##*/}"
    # shellcheck disable=SC1090
    source "$step"
  done
}
