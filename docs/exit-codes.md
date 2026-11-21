# Stable CLI exit codes

The `locus` process exit status is a machine interface. Scripts and bridges
may branch on both the numeric code and `exit_code_name`; both are stable.
Never reuse a number or name for a different meaning. New meanings receive a
new code and name.

<!-- exit-codes:begin -->
| Code | Name | Meaning |
| --- | --- | --- |
| 0 | `success` | success; check commands: healthy (doctor SAFE, agent ready, session ok) |
| 1 | `failure` | command error or refusal; check commands: degraded (doctor WARN, agent protected, session not ok) |
| 2 | `unsafe` | check commands: unsafe (doctor UNSAFE, agent unsafe, status pin invalid/frozen/expired) |
| 2 | `usage` | usage error (bad flag/value, --dry-run on a command that cannot plan) |
| 3 | `refused` | action refused by policy or an authority gate |
| 4 | `repin_required` | human must establish a fresh pin before retrying |
| 5 | `expired` | pin, credential, or authorization expired |
| 6 | `misconfig` | local configuration is invalid or incomplete |
| 126 | `cannot_execute` | child program exists but is not executable (exec/run/ci run) |
| 127 | `not_found` | child program not found (exec/run/ci run) |
| 141 | `stdout_closed` | stdout closed by the reader (128 + SIGPIPE) |
<!-- exit-codes:end -->

The named outcomes 3–6 are reserved for command paths that identify these
conditions specifically. No command emits them yet: today every command error,
including a policy refusal or a missing pin, exits 1 (`failure`), and check
commands use 0/1/2. Do not branch on 3–6 until a release note says a command
returns them.

`unsafe` and `usage` share code 2. Check commands print their report before
exiting 2. A usage error is a clap parse failure (unknown flag or subcommand,
bad or missing value) or `--dry-run` on a command that cannot plan; under
`--json` it prints a `locus.error.v1` envelope with `exit_code_name: "usage"`.

Under `--json` (given before any `--`; after it the flag belongs to the
child), every failing command prints the same `locus.error.v1` envelope
(`schema`, `ok: false`, `error`, `exit_code`, `exit_code_name`) on stderr. It
also prints it on stdout unless the command already wrote its stdout report,
so stdout never holds two documents. `--help` and `--version` are not errors.

`doctor` exits 0 on a `WARN` whose findings are all setup advisories, such as
`phantom_missing`; any other `WARN` exits 1. The report still says `WARN`. See
the [agent contract](./agent-contract.md#exit-codes) for the full list.

`exec`, `run`, `ci run`, and `mcp --supervised` pass through the child status
when it starts (`128 + signal` when killed by a signal). Codes 126 and 127
indicate that the child could not be started (127: not found; 126: found but
not executable, such as a directory or a file without the execute bit). Any
other spawn failure exits 1.
