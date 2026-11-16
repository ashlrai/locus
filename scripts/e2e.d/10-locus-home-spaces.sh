# Exercise the same allocator used at startup, checking its mktemp invocation
# as well as the resulting path. Keep the probe inside the suite's own home.
log "LOCUS_HOME allocation passes a spaced TMPDIR as one argument"
space_root="$LOCUS_HOME/tmp root  with spaces"
mkdir -p "$space_root"
(
  export TMPDIR="$space_root"
  mktemp() {
    [[ "$#" == 2 && "$1" == -d && "$2" == "$TMPDIR/locus-e2e.XXXXXX" ]] || return 1
    command mktemp "$@"
  }
  create_locus_home || exit 1
  [[ -d "$LOCUS_HOME" && "${LOCUS_HOME%/*}" == "$TMPDIR" && \
    "${LOCUS_HOME##*/}" == locus-e2e.* ]] || exit 1
  "$LOCUS_BIN" init >/dev/null || exit 1
  [[ -d "$LOCUS_HOME/bindings" ]] || exit 1
) || die "LOCUS_HOME allocation failed under a space-containing TMPDIR"
ok "LOCUS_HOME is created and initialized beneath the complete TMPDIR path"
