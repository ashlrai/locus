# Signed credential injection audit (opt-in)

Locus can keep a signed, value-free record of every credential it places into a child environment. Off by default: with nothing configured, nothing beyond `config.toml` is read, nothing is written, and injection behaves exactly as before.

```toml
# $LOCUS_HOME/config.toml
[credential_audit]
enabled = true
```

## What a record holds

One JSON line per provider per injection in `$LOCUS_HOME/audit/credential-injections.jsonl`:

| Field | Meaning |
|---|---|
| `v` | record format version, currently `1` |
| `ts` | RFC 3339 time of the injection |
| `session_id`, `binding` | which pinned session and binding |
| `provider` | provider whose credential was injected |
| `target` | `child-env` (a launched child or provider worker) or `ci-export` (the CI export map) |
| `credential_ref` | the reference, for example `env:GH_TOKEN` or `phm:NAME`; anything that does not parse as a reference is logged as `<unrecognized reference>` |
| `env_keys` | names of the environment variables filled, sorted |
| `prev`, `mac` | chain link and signature (below) |

Never recorded: the credential value, any hash or prefix of it, or its length.

## Signing and key handling

- Each record is signed with HMAC-SHA256 under a local key at `$LOCUS_HOME/credential-audit.key`: 32 random bytes as hex, created on first use with mode `0600`. It is separate from the session seal key.
- Each record's `prev` is the previous record's `mac` (the first uses `genesis`), so an edited, inserted, removed or reordered line is detected.
- The key never leaves the machine and is never logged. Back it up together with the log if you need to verify later; without it the log cannot be verified.
- Rotating: move the old key and log aside together, and a new pair starts on the next injection. Mixing a log with a different key fails verification.
- Limits: anyone who can read the key on this machine can forge records. This detects after-the-fact edits; it is not a remote or tamper-proof store.

## Verifying

`locus_core::credential_audit::verify(home)` re-checks every signature and the chain. It returns the number of good records and, on failure, the 1-based line number of the first bad record and the reason (invalid record, broken chain, or signature mismatch). A missing log verifies as empty. There is no `locus` CLI command for verification yet; call the library function.

## Failure behavior

When enabled and a record cannot be written, the credential is withheld from that environment (fail closed) and the provider's `LOCUS_<PROVIDER>_CREDENTIAL_RESOLVED` flag is `0`. What happens next depends on the caller:

- `locus exec`, `locus run` and `locus ci run` report a resolution issue with source `credential_audit` and code `audit_write_failed`, then start the child without the credential. With `--strict-creds` they refuse to start it.
- Provider MCP workers that resolve credentials refuse to launch.
- Routed `locus exec` (a CLI route, see [cli-routing.md](cli-routing.md)) treats the issue as fatal.
- The CI export map (`ci-export`) drops the credential and sets the flag to `0`, but reports no issue.

## Scope

Recorded where Locus builds a child environment with resolved credentials (`build_isolated_env_opts`, used for launched children and provider workers) and in the CI export map when it includes secrets (`build_ci_env_map`, only with `LOCUS_CI_ALLOW_SECRETS=1`). Credential resolution itself is unchanged; references stay references.
