# Durable pin supervisor

`locus supervisor start` starts a detached, long-lived process on macOS and
Linux. It owns the existing authority broker so a pin survives the exit of
the shell that entered it. This is opt-in; ordinary shell-owned brokers keep
their existing lifetime and fail-closed behavior.

Start it from a human-controlled shell with the Locus control capability
already loaded. It never loads that capability automatically, resolves provider
credentials, or puts a bearer in command arguments, logs, or new state files.

```bash
# Use an initialized Locus home. Load the hook in the human shell.
eval "$(locus hook zsh)"          # bash is also supported
locus supervisor start
locus enter personal
locus whoami
```

The shell may now exit. In a later human shell, load the same hook before
`locus whoami`, `locus exec`, or `locus mcp --supervised`. Agents still receive
only delegated executor authority. A durable supervisor does not grant an
agent permission to pin or change accounts, and it does not make a directly
launched `locus-mcp` an executor.

If `init`, `quickstart`, or an earlier pin started a broker in the current
shell, first finish that session and close its owning shell. Start the durable
supervisor from a new hooked shell. Startup refuses to claim a broker owned
by another shell or supervisor; it never silently takes over or revokes one.
Starting it twice also refuses the second start, leaving the first intact.

## Lifetime and recovery

`start --json` returns `locus.supervisor.v1` with `supervisor_pid`, `broker_pid`,
and `epoch`. Success means the helper authenticated the broker and the broker
recorded this supervisor as its owner. Startup neither creates a pin nor
rewrites an existing pin. Use `locus status` / `locus doctor` for pin health.

The supervisor drops its copy of the startup capability after handing it to
the broker helper. It remains idle until the broker exits, its endpoint is
removed or replaced, or the Locus home disappears. It never restarts a broker
or recreates authority. `locus leave` revokes the active pin; the idle
supervisor remains available for the next human pin.

To stop the detached supervisor, terminate the process identified by the
startup result using your OS process controls. With a service manager, stop
the service. Locus deliberately has no command that signals a PID loaded from
disk and no additional management socket or authentication protocol.

After a supervisor dies, the existing broker and every authority check still
reject its PID/start-time/executable identity. Even before broker cleanup,
`whoami` and execution fail closed with `human re-pin required`. A restarted
supervisor starts a new broker epoch; an old sealed pin remains invalid until
the human explicitly runs `locus enter <alias>`. Bad seals or an unverifiable
control capability still require the existing recovery steps.

Pin TTL, maximum lifetime, generation checks, scope freeze, and approvals are
unchanged. The supervisor does not renew pins. Reboot, logout policies that
terminate all user processes, or replacing the supervising executable may
end authority and require human re-entry.

## Service managers

`locus supervisor run` stays in the foreground for launchd or another service
manager. Supply the operator capability through your existing trusted launcher,
outside model context; do not embed its value in a plist or service definition.
Use an absolute Locus binary path and explicit `LOCUS_HOME`. The foreground
process clears its inherited environment before starting the broker helper.

This change installs no service and changes no launchd settings. A service
restart can start a broker but cannot restore the previous pin. Supervision is
only supported on platforms where the existing broker supports process
identity checks (macOS and Linux). `--dry-run` is refused before any startup.
