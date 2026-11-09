# Exercise the actual EXIT trap after the workspace exists and the shell is
# interrupted. Keep the rendezvous outside the child-owned home and workspace.
log "workspace cleanup after interruption"
interrupt_root="$LOCUS_HOME/interruption  test"
mkdir -p "$interrupt_root"
touch "$interrupt_root/keep"
(
  export TMPDIR="$interrupt_root"
  LOCUS_HOME="$interrupt_root/child-home"
  mkdir -p "$LOCUS_HOME"
  WS_DIR="" GRAPH_HOME="" SECURITY_HOME="" SECURITY_WS="" HTTP_MCP_PID="" MT_MCP_PID=""
  trap 'e2e_cleanup' EXIT
  create_workspace
  [[ -d "$WS_DIR" && "${WS_DIR%/*}" == "$TMPDIR" && "${WS_DIR##*/}" == locus-ws.* ]] || exit 1
  touch "$WS_DIR/sentinel"
  printf '%s' "$WS_DIR" > "$interrupt_root/workspace-path"
  while :; do sleep 1; done
) &
interrupt_pid=$!
interrupt_ready=0
for _ in {1..100}; do
  if [[ -s "$interrupt_root/workspace-path" ]]; then
    interrupt_ready=1
    break
  fi
  kill -0 "$interrupt_pid" 2>/dev/null || break
  sleep 0.1
done
if [[ "$interrupt_ready" -ne 1 ]]; then
  kill "$interrupt_pid" 2>/dev/null || true
  wait "$interrupt_pid" 2>/dev/null || true
  die "interruption rehearsal did not create a workspace"
fi
interrupt_workspace="$(cat "$interrupt_root/workspace-path")"
kill -TERM "$interrupt_pid" || die "could not interrupt workspace rehearsal"
interrupt_status=0
wait "$interrupt_pid" || interrupt_status=$?
[[ "$interrupt_status" -eq 143 ]] || die "workspace rehearsal did not terminate from SIGTERM"
[[ ! -e "$interrupt_workspace" && ! -L "$interrupt_workspace" ]] || die "workspace survived interruption"
[[ ! -e "$interrupt_root/child-home" && -f "$interrupt_root/keep" ]] || die "interruption cleanup removed the wrong paths"
ok "interruption removes the workspace and preserves its temporary root"
