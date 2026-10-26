# Route a provider CLI

After a human pins a binding, select one provider for a trusted native CLI:

```bash
locus exec --for gh -- gh pr list
locus exec --for vercel -- vercel list
locus exec --for supabase -- supabase projects list
locus exec --for aws -- aws sts get-caller-identity
```

`--for` accepts these exact CLI names. The command after `--` must have the
same executable name; an absolute path such as `/opt/homebrew/bin/gh` works.
`--expect <alias>` can be combined with `--for`.

| CLI | Binding provider | Credential environment |
| --- | --- | --- |
| `gh` | `github` | `GH_TOKEN`, `GITHUB_TOKEN`, `GITHUB_PERSONAL_ACCESS_TOKEN` (one credential, compatible aliases) |
| `vercel` | `vercel` | `VERCEL_TOKEN` |
| `supabase` | `supabase` | `SUPABASE_ACCESS_TOKEN` |
| `aws` | `aws` | `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, optional `AWS_SESSION_TOKEN` |

Locus verifies the seal, broker authority, expected alias, and drift of the
**whole binding**, then resolves only the selected provider. Unknown CLIs,
missing or duplicate providers, missing/empty/invalid credentials, and invalid
pins refuse before the child starts. Credential failures are always fatal in
this mode, even without `--strict-creds`. `--no-resolve` cannot be combined
with `--for`, and delegated sessions cannot resolve CLI credentials.

Each invocation receives a fresh private HOME and GH/AWS configuration paths,
cleaned up after the child exits. Ambient credentials, profile variables,
credential reference names, `[exec] inherit_env` additions, and Locus control
or executor capabilities are not passed to the child. Other providers' frozen
scope variables are also omitted. An unrelated unavailable credential does
not block the selected route. Child exit codes are preserved, and audit records
identify the routed provider without recording resolved credential values.

## AWS credential bundle

AWS needs a key pair. Set the binding's `credential_ref` to an explicit
`env:VAR` whose value is a JSON object with exactly these fields:

```json
{
  "AWS_ACCESS_KEY_ID": "synthetic-example-id",
  "AWS_SECRET_ACCESS_KEY": "synthetic-example-secret",
  "AWS_SESSION_TOKEN": "synthetic-example-session-token"
}
```

Omit `AWS_SESSION_TOKEN` for long-lived keys. Duplicate, unknown, missing,
non-string, empty, or control-character fields are rejected. Supply real
values outside agent context; never put them in binding TOML or command args.
Stored `phm:` references still fail closed until the scoped bridge described
in [credential compatibility](credential-compatibility.md) is available.

The route disables EC2 metadata credential fallback and configured endpoint
overrides. A pinned `scope.region` supplies `AWS_REGION` and
`AWS_DEFAULT_REGION`. Legacy scalar AWS secrets retain their old mapping for
ordinary exec, but are incomplete and therefore refused by `--for aws`.

## Boundary

This is the existing trusted-process `exec` boundary, narrowed to one
provider. Locus selects credentials; it does not sandbox the executable,
validate every native CLI argument, verify the remote identity of a supplied
credential, or translate arbitrary commands into MCP policy checks. Native
CLI flags and repository-local configuration remain under the caller's
control. Frozen scope environment values alone cannot enforce an API scope.
Use scoped credentials and the supervised MCP path when tool-level scope
freeze and approval enforcement are required. Do not put credentials in CLI
arguments: exec's existing audit records include caller-supplied arguments.

References: [AWS CLI environment variables](https://docs.aws.amazon.com/cli/latest/userguide/cli-configure-envvars.html),
[Vercel CLI token environment support](https://github.com/vercel-labs/agent-skills/blob/main/skills/vercel-cli-with-tokens/SKILL.md).
