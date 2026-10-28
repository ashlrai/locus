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
| 2 | `usage` | usage error (bad flag/value, --dry-run on a command that cannot plan); no report on stdout |
| 3 | `refused` | action refused by policy or an authority gate |
| 4 | `repin_required` | human must establish a fresh pin before retrying |
| 5 | `expired` | pin, credential, or authorization expired |
| 6 | `misconfig` | local configuration is invalid or incomplete |
| 126 | `cannot_execute` | child program exists but is not executable (exec/run/ci run) |
| 127 | `not_found` | child program not found (exec/run/ci run) |
| 141 | `stdout_closed` | stdout closed by the reader (128 + SIGPIPE) |
<!-- exit-codes:end -->

The named outcomes 3–6 are reserved for command paths that identify these
conditions specifically. Existing generic command errors continue to return
1, and check commands continue to use 0/1/2. In particular, `unsafe` and
`usage` share code 2: check commands print their report before exiting, while
usage errors print no report on stdout. Under `--json`, usage errors include
`exit_code_name: "usage"` on stderr.

`exec`, `run`, `ci run`, and `mcp --supervised` pass through the child status
when it starts (`128 + signal` when killed by a signal). Codes 126 and 127
indicate that the child could not be started.
