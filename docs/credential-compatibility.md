# Credential compatibility

This source checkout uses `env:VAR` for supported credential resolution. A binding stores only the variable name; a trusted operator supplies its value outside agent context before supervised execution. Locus scrubs ambient identity and injects only the pinned binding’s named provider keys. Never put values into binding files, agent prompts, or logs.

For example, a GitHub binding can contain this pointer:

```toml
[[providers]]
provider = "github"
account = "replace-with-account"
credential_ref = "env:LOCUS_CLIENT_A_GITHUB_TOKEN"
scope = { orgs = ["replace-with-organization"] }
```

Supply the named variable through your operator-controlled environment. Review the account and frozen scopes, enter the binding, then use `locus whoami` before supervised execution. Missing environment variables remain unresolved; Locus never substitutes ambient account credentials. CI credential resolution retains its existing explicit opt-in gates.

## Phantom Secrets references

`phm:NAME` remains a valid stored reference for metadata, migration and a future integration. It does not currently resolve a usable credential. Resolution returns an unsupported-integration error before invoking Phantom, and credential-resolving CLI launches fail before child or session effects. Doctor reports the same fixed issue code even when Phantom is installed or the referenced name exists. Credential-free metadata and `--no-resolve` operations retain their existing rules.

Secrets v0.7.9 protects `reveal` with a trusted-terminal boundary. The legacy Locus adapter’s `phantom reveal --yes NAME` call is incompatible with that release. Do not bypass the boundary, capture reveal output, or substitute Secrets `env`/`unwrap` commands: those commands generate env examples and undo package wrappers, respectively.

A supported integration needs a separately reviewed, tenant-scoped, value-blind execution contract. Installation, stored reference presence, and a readable Secrets configuration do not establish that contract or grant execution authority. No bridge or runtime is activated by this change.

The v0.5.2 source tag already contains this fail-closed correction. Published Locus v0.5.0 contains the legacy adapter; this behavior is not newly introduced by the 0.6.0 release. Existing `phm:` bindings are preserved for owner review rather than automatically rewritten.

## Configuration validation

Phantom reference names follow the same grammar as the metadata listing probe:
`[A-Z_][A-Z0-9_]*`, at most 256 ASCII characters. Invalid names are rejected by
binding validation and are never automatically migrated from bare names.

MCP upstream specifications reject unknown fields. In particular, `env`,
`headers`, and `credentials` tables are not supported places to store literal
credentials; use the provider's explicit `credential_ref` instead. This schema
check does not inspect arbitrary command arguments for secret material. Never
embed credentials in an upstream command or its arguments.
