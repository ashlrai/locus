# Dry-run review matrix (help surface)

[Issue #171](https://github.com/ashlrai/locus/issues/171) reported that most
mutating commands had no side-effect-free preview; the global `--dry-run`
flag is the answer. This page audits the flag's **documentation surface**:
for every command form reachable from `locus --help` in this checkout,
whether `locus <form> --help` lists the flag. The
[command matrix](dry-run-matrix-q8.md) describes what the flag does once
accepted and the [acceptance checklist](dry-run-acceptance.md) how to review
a change; this matrix records what the help advertises. The tables are
re-derived from the built binary by
`crates/locus-cli/tests/dry_run_review_matrix.rs`, so they cannot drift from
the code.

## Inspected help

Binary: `locus 0.6.0` (`cargo build -p locus-cli` in this checkout). Method:
run `locus <form> --help` for every command form in the tree and record
whether the output contains a `--dry-run` option line.

`locus --help` defines the flag once, as a global option:

```text
      --dry-run
          Print the planned changes of a mutating command without writing (read-only commands accept it and change nothing; see `locus topic agent-contract`)
```

Every documenting form repeats that global line in its own help, inline in
the options list — `locus pin --help`:

```text
      --dry-run  Print the planned changes of a mutating command without writing (read-only commands accept it and change nothing; see `locus topic agent-contract`)
```

An invocable form's help documents the flag exactly when the form accepts
it (grouping pages are covered separately below). The child/server forms
never had a `--dry-run` flag, so `parse_cli`
(`crates/locus-cli/src/main.rs`) removes it from those command trees before
rendering; `locus exec --help` then lists only

```text
Options:
      --json  Emit machine-readable JSON where applicable (versioned: `schema` key)
  -h, --help  Print help
```

ahead of its own options. The same tree drives clap's `help` subcommand, so
`locus help <form>` shows the same option list as `locus <form> --help`
(`locus help topic` is the exception noted under Aliases).

## Help documents `--dry-run`

Names are command forms, not complete invocations; required arguments are in
each form's help. **plan** means the flag prints a `locus.plan.v1` preview
and writes nothing; **accepted, ignored** means the read-only command runs
exactly as without it (see the [agent contract](agent-contract.md#--dry-run)
for the side effects it keeps).

| Command forms | With `--dry-run` |
| --- | --- |
| `init`, `quickstart`, `onboard`, `workspace`, `enter`, `switch`, `leave`, `renew` | plan |
| `setup` | plan (`--print` and `--client generic` print config instead) |
| `pin` | plan (rejected with `--rewrite-configs`) |
| `engagement init`, `engagement close`, `graph export`, `graph import`, `binding add`, `binding rm`, `binding migrate-credential-refs`, `client add`, `offboard` | plan |
| `autopin suggest` | plan only with `--apply`; accepted, ignored without |
| `capability persist`, `capability unpersist`, `ci mint`, `ci env`, `mcp mint`, `mcp revoke` | plan |
| `agent setup`, `adapter install`, `adapter uninstall`, `adapter trust add`, `adapter registry export`, `adapter registry index add`, `adapter registry index remove` | plan |
| `doctor` | plan only with `--fix`; accepted, ignored without |
| `approve grant`, `approve deny`, `notify on`, `notify off`, `events export`, `forensics export` | plan |
| `whoami`, `status`, `watch`, `hook`, `completion`, `topic`, `version`, `fleet-preflight`, `fleet-feed`, `fleet-denials`, `events` | accepted, ignored |
| `agent doctor`, `agent report`, `binding list`, `binding show`, `graph list`, `capability status`, `notify status`, `approve list`, `approve status`, `approve wait`, `mcp list`, `goal status`, `verify session`, `verify claim`, `upstream list`, `upstream suggest` | accepted, ignored |
| `adapter search`, `adapter list`, `adapter verify`, `adapter verify-manifest`, `adapter trust list`, `adapter registry index list` | accepted, ignored |

`binding migrate-credential-refs`, `doctor --fix` and `offboard` already
preview without the flag; `--dry-run` produces the plan schema. Combining
`--apply` with `--dry-run` is a usage error on `doctor --fix` and `offboard`
(exit 2) and on `agent setup` (exit 1).

## Help does not document `--dry-run`

| Command form | Why the flag is absent |
| --- | --- |
| `exec` | runs a command in the pinned session |
| `run` | runs a command with a binding |
| `ci run` | runs a command in CI |
| `serve` | local server |
| `dashboard` | local dashboard server |
| `mcp` (no subcommand) | MCP stdio server |

These forms run a child or a server rather than plan changes, so the flag is
absent from their option list and rejected when given. A top-level
`locus --dry-run <form>` fails with `unexpected argument '--dry-run' found`
(exit 2). Placed after the subcommand, `exec`, `run` and `ci run` first fail
their own argument check — `locus exec --dry-run -- true` exits 1 with
"`--dry-run` is not a `locus exec` option, and a command cannot start with
`-`" — while `serve`, `dashboard` and `mcp` exit 2. `mcp` also lists `mint`,
`list` and `revoke` subcommands; those are separate forms in the
documents table above.

## Grouping help pages

These forms group subcommands and are not invocable on their own — their
usage line requires `<COMMAND>`. As grouping pages they still list the
global flag, even where a child form does not: `locus ci --help` documents
`--dry-run` while `locus ci run --help` does not.

| Command forms |
| --- |
| `capability`, `autopin`, `engagement`, `graph`, `agent`, `goal`, `verify`, `binding`, `client`, `upstream` |
| `adapter`, `adapter trust`, `adapter registry`, `adapter registry index`, `ci`, `approve`, `notify`, `forensics` |

`mcp` is a grouping page too, but the bare form runs the server and sits in
the absent table instead; `events` is invocable without a subcommand and
sits in the documents table.

## Aliases

Aliases render their target's help, option list included: `approve pending`
shows `approve list` help, and `help-topic` shows `topic` help. Clap's
implicit `locus help` prints the top-level help, which documents the flag.
`locus help topic` is the one help-render exception: it is intercepted to
list the extended-help topics rather than render `topic`'s options.

## Review notes

- A documenting form's `--dry-run` acceptance is not authorization: a
  successful plan does not authenticate the control capability, start a
  broker, or resolve credentials. See
  [agent-contract.md](agent-contract.md#--dry-run).
- Read-only commands keep two side effects under `--dry-run`: opening the
  store can create `LOCUS_HOME`, and session inspection can freeze a
  drifted pin as a fail-closed security measure.
- To re-run this audit manually: `locus --help`, then `locus <form> --help`
  for each form; or run the `dry_run_review_matrix` tests
  (`cargo test -p locus-cli --test integration`). The
  [manual verification procedure](dry-run-verification.md) checks that
  previews also leave state unchanged.
