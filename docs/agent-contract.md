# Agent contract: JSON schemas, exit codes, `--dry-run`

The `locus` CLI is driven by agents and CI as often as by people. This page is
the machine contract: what `--json` prints, what each exit code means, how
`--dry-run` previews a change, and when the CLI will (never) prompt.

The same facts are available from the binary itself:

```bash
locus topic agent-contract          # human table
locus topic agent-contract --json   # locus.contract.v1
```

`crates/locus-cli/tests/agent_contract.rs` checks this page against that
output, so the tables below cannot drift from the code.

## JSON schemas

With `--json`, every check, status and plan command prints one JSON object
whose first key is `schema`. Key on it:

- The id is `locus.<surface>.v<N>`. `N` changes only when the shape breaks
  (a field is removed, renamed or retyped). New fields are additive and keep
  the id, so a reader must ignore keys it does not know.
- The rest of the object is the command's existing payload. Older fields such
  as `schema_version`, `kind` and `version` stay for older readers.
- No document carries a secret, a control-capability value or a credential.
  `locus status` and `locus watch` print one line; the others are pretty JSON.
- MCP tool payloads and the dashboard `/api/*` responses are not stamped.

| Command | Schema |
| --- | --- |
| `locus doctor` | `locus.doctor.v1` |
| `locus doctor --fix [--apply]` | `locus.doctor.fix.v1` |
| `locus whoami` | `locus.whoami.v1` |
| `locus status` | `locus.status.v1` |
| `locus verify session` | `locus.verify.session.v1` |
| `locus verify claim` | `locus.verify.claim.v1` |
| `locus agent doctor` | `locus.agent.doctor.v1` |
| `locus agent report` | `locus.agent.report.v1` |
| `locus watch` (one NDJSON line per tick) | `locus.watch.v1` |
| `locus goal status` | `locus.goal.status.v1` |
| `locus capability status` | `locus.capability.status.v1` |
| `locus notify status` | `locus.notify.status.v1` |
| `locus approve status` | `locus.approve.status.v1` |
| `locus autopin suggest` | `locus.autopin.suggest.v1` |
| `locus adapter verify` | `locus.adapter.verify.v1` |
| `locus adapter verify-manifest` | `locus.adapter.verify_manifest.v1` |
| `locus version` | `locus.version.v1` |
| any mutating command with `--dry-run` | `locus.plan.v1` |
| any command failing under `--json` (stderr) | `locus.error.v1` |
| `locus topic agent-contract` | `locus.contract.v1` |
| `locus --ashlr-handshake` | `locus.ashlr.handshake.v1` |

List commands (`binding list`, `events`, `approve list`, `mcp list`,
`graph list`, `upstream list`) print a bare JSON array, unchanged, and are not
stamped: wrapping them would break existing readers.

### Errors

Under `--json`, a failing command keeps stdout clean (or prints only its
report, as `verify session` does before exiting 1) and prints one line on
stderr:

```json
{"schema":"locus.error.v1","ok":false,"error":"no active pin …","exit_code":1,"exit_code_name":"failure"}
```

Usage errors (unknown flag, bad value) use the same shape with
`"exit_code":2,"exit_code_name":"usage"`. The message is the same text as the
human `error:` line and never contains a secret.

## Exit codes

One table for every command, defined once in
`crates/locus-cli/src/exit.rs` (`ExitCode`). Check commands map their verdict
onto it: doctor SAFE/WARN/UNSAFE and agent `ready`/`protected`/`unsafe` give
0/1/2.

<!-- exit-codes:begin -->
| Code | Name | Meaning |
| --- | --- | --- |
| 0 | `success` | success; check commands: healthy (doctor SAFE, agent ready, session ok) |
| 1 | `failure` | command error or refusal; check commands: degraded (doctor WARN, agent protected, session not ok) |
| 2 | `unsafe` | check commands: unsafe (doctor UNSAFE, agent unsafe, status pin invalid/frozen/expired) |
| 2 | `usage` | usage error (bad flag/value, --dry-run on a command that cannot plan); no report on stdout |
| 3 | `refused` | action refused by policy or an authority gate |
| 4 | `repin_required` | human must establish a fresh pin before retrying |
| 5 | `expired` | pin, credential, or authorization expired |
| 6 | `misconfig` | local configuration is invalid or incomplete |
| 126 | `cannot_execute` | child program exists but is not executable (exec/run/ci run) |
| 127 | `not_found` | child program not found (exec/run/ci run) |
| 141 | `stdout_closed` | stdout closed by the reader (128 + SIGPIPE) |
<!-- exit-codes:end -->

`unsafe` and `usage` share 2, as clap's usage errors always have. Tell them
apart by stdout: a check command prints its report before exiting 2, a usage
error prints nothing on stdout (and `exit_code_name: "usage"` on stderr under
`--json`).

`exec`, `run`, `ci run` and `mcp --supervised` exit with the child's own
code (`128 + signal` when a signal killed it); only 126 and 127 come from
this table, when the child never started.

Per command:

| Command | 0 | 1 | 2 |
| --- | --- | --- | --- |
| `doctor` | SAFE | WARN, or a refused Phantom preflight | UNSAFE |
| `agent doctor`, `agent report` | `ready` | `protected` | `unsafe` |
| `status` | unpinned, or pin ok | error | pin invalid, frozen or expired |
| `verify session` | `session_ok` | not ok | usage |
| `watch --once` / `--require-ok` | ok | not ok (see `locus watch --help`) | usage |
| `quickstart` | SAFE or WARN | error | UNSAFE |
| `doctor --fix --apply` | all fixes applied (or none needed) | a fix was refused or skipped | usage |
| any `--dry-run` | the plan's read-only preconditions hold | a precondition fails (unknown alias, allowlist, existing file) | `--dry-run` not accepted by the command (see Unsupported), or combined with `--apply` on `doctor --fix` |

## `--dry-run`

`--dry-run` is a global flag. Every command is in exactly one class, enforced
by an exhaustive match in `dry_run_support` (`crates/locus-cli/src/main.rs`):
a new command does not compile until it picks one.

| Class | Behaviour | Commands |
| --- | --- | --- |
| Plans | Runs the read-only validation of the real command, prints a `locus.plan.v1` plan, writes nothing | `init`, `quickstart`, `onboard`, `setup`, `workspace`, `enter`, `switch`, `pin`, `leave`, `renew`, `engagement init/close`, `graph export/import`, `binding add/rm/migrate-credential-refs`, `client add`, `autopin suggest --apply`, `capability persist/unpersist`, `ci mint/env`, `mcp mint/revoke`, `agent setup`, `doctor --fix`, `adapter install/uninstall`, `adapter trust add`, `adapter registry export`, `adapter registry index add/remove`, `approve grant/deny`, `notify on/off`, `events export`, `forensics export` |
| Read-only | Accepted and ignored: the command runs exactly as without the flag (see below) | `whoami`, `status`, `doctor`, `watch`, `hook`, `goal`, `verify`, `upstream`, `completion`, `topic`, `version`, `events`, `agent doctor/report`, `binding list/show`, `graph list`, `capability status`, `notify status`, `approve list/status/wait`, `mcp list`, `autopin suggest`, `adapter search/list/verify/verify-manifest`, `adapter trust list`, `adapter registry index list` |
| Unsupported | Not accepted, exactly as before the global flag: `exec --dry-run -- …` fails exec's own argument check (exit 1); every other placement, and the other commands, are a clap usage error (exit 2, `unexpected argument '--dry-run' found`) | `exec`, `run`, `ci run`, `mcp` (server), `serve`, `dashboard` |

Read-only commands make no requested change, but they keep two side effects
of a normal run under `--dry-run`: opening the store creates `LOCUS_HOME` on
first use, and `whoami`, `status`, `doctor`, `agent doctor/report`,
`verify session` and `watch` still freeze the active pin when its binding
drifted. The drift freeze is a fail-closed security control, so `--dry-run`
never suppresses it.

What a plan does and does not check:

- It resolves targets through the same code as the real command. `pin`,
  `enter`, `switch`, `ci mint` and `mcp mint` call `Store::resolve_pin` (the
  binding, workspace allowlist, namespaces and TTL cap the real pin uses);
  `engagement init` calls `Store::engagement_init_preflight`; `graph import`
  decrypts through `Store::graph_import_preview`; `doctor --fix` lists the
  fixes `--apply` would make.
- It never opens a store that does not exist yet (opening creates the home
  and its seal key). Against an uninitialized home, `init`, `quickstart`,
  `agent setup` and `binding add` plan the creation; the others fail with a
  pointer to `locus init --dry-run`.
- It does **not** authenticate the control capability: that would start the
  authority broker, itself a side effect. A plan reports
  `requires_control_capability` and `control_capability_in_env` (presence
  only, never the value). A plan that exits 0 can still be refused by the
  real run's control gate.
- It never prints a secret: no capability, bearer token, MCP grant token or
  resolved credential. `events export --sink webhook` names only the
  webhook's scheme and host.
- `--apply` with `--dry-run` is a usage error (exit 2) on `doctor --fix`. On
  `agent setup` it keeps its original exit 1 (`pass only one of --apply or
  --dry-run`).

Plan shape:

```json
{
  "schema": "locus.plan.v1",
  "ok": true,
  "dry_run": true,
  "command": "locus pin acme",
  "changes": [
    { "action": "unpin", "target": "personal", "detail": "session ses_… ends" },
    { "action": "pin", "target": "acme", "detail": "tenant=acme-corp ttl=8h (max_ttl) providers=3" },
    { "action": "overwrite", "target": "/Users/me/.locus/sessions/active.json", "detail": "sealed session" }
  ],
  "requires_control_capability": true,
  "control_capability_in_env": true,
  "notes": [],
  "binding": "acme",
  "tenant": "acme-corp",
  "ttl_secs": 28800
}
```

`action` is one of `create`, `overwrite`, `update`, `delete`, `pin`,
`unpin`, `mint`, `revoke`, `grant`, `deny`, `register`, `send`, `print`.
Fields after `notes` are command-specific and can never shadow the standard
ones. `changes` may be empty (for example `leave` with no pin): the plan is
then "nothing to do", still exit 0.

## Non-interactive by default

`locus` prompts only when all of these hold:

- stdin is a terminal (stdout does not matter: `locus binding add | tee log`
  still prompts; agents and CI have no terminal on stdin, or pass `--json`
  or `LOCUS_NON_INTERACTIVE=1`);
- `--json` is not set;
- `LOCUS_NON_INTERACTIVE` is unset (or `0`).

Otherwise a missing value fails closed with the flag to pass (exit 1),
confirmations take their safe default (`adapter uninstall` without `--yes`
declines), and `graph export/import` require `LOCUS_GRAPH_PASSPHRASE`.
Destructive commands keep their explicit flags (`binding rm --yes`,
`adapter install --yes`) whether or not a terminal is present.
