# Exercise a real graph import with both the archive and destination under a
# spaced temporary root. The subshell owns its cleanup, including on failure.
log "graph import uses and cleans a space-containing temporary home"
graph_space_root="$LOCUS_HOME/graph import temp root"
mkdir -p "$graph_space_root"
touch "$graph_space_root/keep"
(
  export TMPDIR="$graph_space_root"
  LOCUS_HOME="$graph_space_root/source home"
  mkdir -p "$LOCUS_HOME"
  WS_DIR="" GRAPH_HOME="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
  trap 'e2e_cleanup' EXIT
  export LOCUS_GRAPH_PASSPHRASE="e2e-test"
  "$LOCUS_BIN" init --with-samples >/dev/null || exit 1
  "$LOCUS_BIN" graph export --out "$graph_space_root/export graph.locusgraph" >/dev/null || exit 1
  create_graph_home || exit 1
  [[ -d "$GRAPH_HOME" && "${GRAPH_HOME%/*}" == "$TMPDIR" && \
    "${GRAPH_HOME##*/}" == locus-graph-imp.* ]] || exit 1
  printf '%s' "$GRAPH_HOME" >"$graph_space_root/import-home-path"
  LOCUS_HOME="$GRAPH_HOME" "$LOCUS_BIN" init >/dev/null || exit 1
  LOCUS_HOME="$GRAPH_HOME" "$LOCUS_BIN" graph import "$graph_space_root/export graph.locusgraph" >/dev/null || exit 1
  LOCUS_HOME="$GRAPH_HOME" "$LOCUS_BIN" binding show personal >/dev/null || exit 1
) || die "graph import failed beneath a space-containing TMPDIR"
graph_space_home="$(cat "$graph_space_root/import-home-path")"
[[ ! -e "$graph_space_home" && ! -L "$graph_space_home" && -f "$graph_space_root/keep" ]] \
  || die "graph import cleanup failed or removed unrelated files"
ok "graph import used one spaced path and cleaned its temporary home"
