# Engine broker allowlists

Operators can constrain swarm engines independently of tenant bindings. Create
`$LOCUS_HOME/broker.toml` (default `~/.locus/broker.toml`), owned and writable only
by the operator:

```toml
[engines.codex-personal]
cli = ["gh", "cargo"]
mcp = ["github"]

[engines.reviewer]
cli = []
mcp = ["github"]
```

Launch each engine with `LOCUS_ENGINE_ID` set outside model control. With a
policy file present, missing and unknown engine IDs are denied. With an engine
ID present, missing, unreadable, and malformed policies deny access too. Existing
non-broker installations (no policy and no engine ID) retain their behavior.
An empty engine entry permits nothing; unknown fields reject the entire file.
Changes are read at every gate, so revocations apply to subsequent calls.

`locus exec` checks the exact executable argument before resolving credentials or
launching a child. Paths are distinct from bare executable names; no wildcards
or argument matching are supported. MCP provider calls check the provider server
prefix: `github.list_repos` and `client__github.list_repos` require `mcp =
["github"]`. Control tools remain available for diagnostics and human pin
requests. Existing session seals, scope freezes, policy and approval gates still
apply after an allow decision. Engine IDs propagate through supervised launches.

Check a decision without executing anything:

```sh
locus broker check codex-personal cli:gh --json
locus broker check codex-personal mcp:github --json
```

The JSON contains `allowed` and `reason` (`allowed`, `unknown_engine`, or
`not_allowlisted`). Denied checks exit nonzero. Invalid or missing configuration
also exits nonzero. Runtime denials append `broker.deny` to the existing audit
log using fixed labels only: no arguments, engine identifiers, executable names,
or resolved secrets are recorded. Audit write failure also prevents execution.

This is a launch and proxy boundary, not an OS sandbox. Only allow trusted
executables: shells, interpreters, build tools and extensible CLIs can execute
other programs. Bare names depend on the configured PATH; use operator-controlled
paths where executable identity matters. Keep the policy, LOCUS_HOME and launch
environment outside engine write control. An engine ID is operator attribution,
not a cryptographic identity. Direct upstream access, `locus run`/CI entry points,
and arbitrary child subprocesses are outside this gate; constrain those through
the orchestrator and OS sandbox. Use one MCP process per engine; HTTP clients of
the same process share its engine attribution.
