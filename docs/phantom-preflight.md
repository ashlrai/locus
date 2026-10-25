# Phantom preflight in `locus doctor`

Opt-in. Set `LOCUS_PHANTOM_PREFLIGHT` to run `phantom agent doctor --json --strict`
(phantom-secrets `docs/agent-preflight-contract.md`, schema 1) as part of `locus doctor`.

| Value | Refuses on |
| --- | --- |
| unset (default) | never runs |
| `1`, `true`, `on` | `blocked` |
| `strict` | anything but `ready` |

A refusal turns a SAFE doctor run into exit 1; WARN and UNSAFE keep their exit codes.
A missing phantom, or one without `agent doctor --json`, is only a warning.
`--json` output gains `phantom_preflight: {verdict, blocked_by, refuse, message}`.
Only the verdict and kebab-case check ids are shown, never names or values.
Phantom reads project files and env vars only; it does not open the vault.
