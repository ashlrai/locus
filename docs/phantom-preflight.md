# Phantom preflight in `locus doctor`

Opt-in. Set `LOCUS_PHANTOM_PREFLIGHT` to run `phantom agent doctor --json --strict`
(phantom-secrets `docs/agent-preflight-contract.md`, schema 1) as part of `locus doctor`.

| Value (trimmed, case-insensitive) | Refuses on |
| --- | --- |
| unset, or any other value (default) | never runs |
| `1`, `true`, `on` | `blocked` |
| `strict` | anything but `ready` (`degraded`, `blocked`, `unknown`) |

A refusal makes doctor exit 1 when it would otherwise exit 0: a SAFE run, or a
WARN run whose only warnings are setup advice (such as `approvals_missing`).
Other WARN runs already exit 1, and UNSAFE keeps exit 2.

The preflight is skipped, with a warning and no refusal, when phantom is not on
`PATH`, or when its stdout is not a `phantom.agent.preflight` schema 1 document
with a known verdict and an exit status of 0 to 3 (an older phantom without
`agent doctor --json`).

Human output adds one line: `phantom preflight: ready`, or the verdict (plus
`continuing` when it does not refuse) and a pointer to `phantom agent doctor`.
`--json` output gains `phantom_preflight: {verdict, blocked_by, refuse, message}`;
`verdict` is null when skipped, and `blocked_by` holds the check ids from phantom's
`wrap.blocked_by`. A refusal also adds a `phantom-preflight` fail check and a `fail`
status to the [`ashlr.doctor` wrapper](ashlr-handshake.md).
Only the verdict and kebab-case check ids are shown, never names, values or phantom's text.
Phantom reads project files and env vars only; it does not open the vault.
