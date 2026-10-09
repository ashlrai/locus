# Writing a provider adapter

> **SDK guide:** [adapter-sdk.md](./adapter-sdk.md) · **template:** [examples/adapters/_template/](../examples/adapters/_template/) · **catalog:** [adapters/manifest.toml](../adapters/manifest.toml) · **schema:** [schema/adapter-manifest.schema.json](../schema/adapter-manifest.schema.json) · **CLI:** `locus adapter list` / `locus adapter verify`

Adapters are the **only** place provider-specific knowledge should live in Locus. They define:

1. Which **tools** appear when a binding includes that provider  
2. How **scope freeze** rejects model-supplied account selectors  
3. How tool calls are answered (today: identity/scope stubs; later: workers / upstream MCP)

Canonical design: [DESIGN.md §8](../DESIGN.md) (adapter model) and §9 (threat model).

## Where adapters live (today)

Phase 1 adapters are in-tree Rust modules:

```
crates/locus-core/src/adapters/
  mod.rs          # ProviderAdapter trait, freeze helpers, dispatch, control tools
  supabase.rs
  github.rs
  vercel.rs
```

Registration is a match arm in `adapter_for()`:

```rust
pub fn adapter_for(provider: &str) -> Option<Box<dyn ProviderAdapter>> {
    match provider.to_ascii_lowercase().as_str() {
        "supabase" => Some(Box::new(SupabaseAdapter)),
        "github" => Some(Box::new(GithubAdapter)),
        "vercel" => Some(Box::new(VercelAdapter)),
        // "cloudflare" => Some(Box::new(CloudflareAdapter)),
        _ => None,
    }
}
```

Unknown providers still get a generic `{provider}.scope` identity tool via `tools_for_binding` — useful for experimentation, but a real adapter should own freeze rules.

## The trait

```rust
pub trait ProviderAdapter: Send + Sync {
    fn name(&self) -> &'static str;

    fn tools(
        &self,
        provider: &ProviderBinding,
        binding: &Binding,
    ) -> Vec<AdapterTool>;

    fn call(
        &self,
        tool: &str,
        args: &Value,
        provider: &ProviderBinding,
        binding: &Binding,
    ) -> Result<ToolCallResult>;
}
```

`AdapterTool` fields:

| Field | Purpose |
|-------|---------|
| `name` | MCP tool name — convention `provider.action` (e.g. `supabase.scope`) |
| `description` | Model-facing; **include frozen scope** so the agent sees the fence |
| `input_schema` | JSON Schema object for arguments |
| `provider` | Provider id string |
| `destructive` | Hint for policy / UX; still enforce with `require_approval` globs |

## Scope freeze (required)

Account selectors must not be smuggled through tool args. Use the shared helper:

```rust
use super::freeze_string_arg;

// In call():
let frozen = provider.scope.project_ref.as_deref();
let project_ref = freeze_string_arg(args, "project_ref", frozen)?;
// Err if model sends a different project_ref when frozen is set
```

| Provider | Typical frozen knobs |
|----------|----------------------|
| Supabase | `project_ref`, `read_only` |
| GitHub | `orgs[]`, `repos[]` |
| Vercel | `team_id`, projects, env (preview/prod) |
| Anthropic | `account_id` (org id), `project_ref` (workspace id), `read_only` — per-tenant model-API spend isolation; aliases `org`/`org_id`/`organization`/`workspace`/`workspace_id` (+ camelCase) are freeze-netted |
| OpenAI | `account_id` (org id), `project_ref` (project id), `read_only` — per-tenant model-API spend isolation; aliases `org`/`org_id`/`organization`/`project`/`project_id` (+ camelCase) are freeze-netted |
| Cloudflare (future) | `account_id`, zones |
| AWS (future) | account / region / role |

**Rule:** if the binding freezes a selector, model-supplied mismatch → **error**, not warn.

## Tool naming and dispatch

- Tools must be prefixed with the provider name: `supabase.table.delete`.
- `call_tool` in `mod.rs` routes by the first segment before `.`.
- Policy runs **before** adapter `call` (deny / require_approval / allow).
- Destructive stubs should require `confirm: true` when matched by `require_approval` globs (e.g. `*.delete*`).

## What not to do

- **Do not** return `credential_ref` strings or resolved secret values in tool content. Scope/identity responses may return only safe credential presence/source metadata or a digest.
- **Do not** fall through to ambient `gh auth`, global AWS profile, or another binding’s env.
- **Do not** call remote APIs with credentials resolved outside the pinned binding’s refs (when you add live calls).
- **Do not** print to stdout from MCP-adjacent paths — pollutes the MCP stream.

## Step-by-step: new adapter

1. **Define scope fields** you will freeze (extend `Scope` in `binding.rs` if needed; keep serde defaults so old TOMLs still load).
2. **Create** `adapters/myprovider.rs` with a unit struct implementing `ProviderAdapter`.
3. **Expose** at least:
   - `myprovider.scope` — identity dump of frozen knobs (no secrets)
   - optional health/whoami-style tools
4. **Implement** `call` with freeze on every selector.
5. **Register** in `adapter_for` and `mod myprovider`.
6. **Tests** in `adapters/mod.rs` or the new module:
   - freeze rejects wrong selector
   - happy path returns frozen scope
   - policy blocks a destructive tool without `confirm`
7. **Docs**: one row in the matrix below; binding example if user-facing.
8. **Credential confinement**: provider credentials may resolve into isolated `locus exec`, `locus run`, and `locus ci run` children by default. Use their shared `--no-resolve` mode for identity-only diagnostics; it rejects recipe-expanded resolving upstreams before effects. CI `mint/env --resolve` additionally requires `LOCUS_CI_ALLOW_SECRETS=1`. Never return credentials through MCP results or logs.

### Minimal skeleton

```rust
use super::{freeze_string_arg, AdapterTool, ProviderAdapter, ToolCallResult};
use crate::binding::{Binding, ProviderBinding};
use crate::error::Result;
use serde_json::{json, Value};

pub struct CloudflareAdapter;

impl ProviderAdapter for CloudflareAdapter {
    fn name(&self) -> &'static str {
        "cloudflare"
    }

    fn tools(&self, provider: &ProviderBinding, binding: &Binding) -> Vec<AdapterTool> {
        let account = provider
            .scope
            .account_id
            .as_deref()
            .unwrap_or("<unset>");
        vec![AdapterTool {
            name: "cloudflare.scope".into(),
            description: format!(
                "Frozen Cloudflare scope for `{}` / `{}`: account_id={account}",
                binding.tenant, binding.alias
            ),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "account_id": { "type": "string" }
                },
                "additionalProperties": false
            }),
            provider: "cloudflare".into(),
            destructive: false,
        }]
    }

    fn call(
        &self,
        tool: &str,
        args: &Value,
        provider: &ProviderBinding,
        binding: &Binding,
    ) -> Result<ToolCallResult> {
        let frozen = provider.scope.account_id.as_deref();
        let account_id = freeze_string_arg(args, "account_id", frozen)?;
        match tool {
            "cloudflare.scope" => Ok(ToolCallResult {
                ok: true,
                content: json!({
                    "provider": "cloudflare",
                    "account": provider.account,
                    "account_id": account_id,
                    "tenant": binding.tenant,
                    "binding": binding.alias,
                }),
                policy: None,
            }),
            other => Err(crate::error::LocusError::msg(format!("unknown tool {other}"))),
        }
    }
}
```

*(Adjust `Scope` fields to match the real struct in this repo — do not invent fields without updating `binding.rs`.)*

## Binding TOML example

```toml
[[binding.providers]]
provider = "supabase"
account = "acme-prod"
credential_ref = "env:LOCUS_SUPABASE_ACME"
scope = { project_ref = "abcdefghij", read_only = true }
```

Never put raw secrets in binding files — only CredentialRefs (`phm:` or `env:`). Production rejects `test:`.

## Phase 1 vs later

| Phase 1 (now) | Phase 2+ (roadmap) |
|---------------|---------------------|
| Synthetic tools, local freeze | Real upstream MCP / REST workers |
| In-tree `ProviderAdapter` | Optional out-of-tree adapter packages / registry |
| Policy stubs with `confirm` | Approval UX, dual-control, TTL elevation |

Prefer **wrapping** official upstream MCP servers with frozen env over reimplementing APIs — see DESIGN §8.3.

## Checklist before merge

- [ ] Freeze tests for every hard scope knob  
- [ ] No secrets in tool responses  
- [ ] Destructive tools covered by policy globs or explicit gates  
- [ ] `cargo test -p locus-core` green  
- [ ] `cargo clippy -p locus-core -- -D warnings` clean  

## Community adapter marketplace (unreleased candidate)

Community adapters are publisher-supplied **executable code**. A trusted
signature binds the complete installable envelope to a configured verification key;
it does not establish publisher identity or that a command is safe. Review the command, ordered args,
credential mapping and sandbox flags before approving installation. Discovery
summaries are untrusted metadata and do not authorize execution.

```bash
locus adapter registry index add https://adapters.example.com/index.json --name curated
locus adapter trust add --id example-publisher --ed25519-pub <base64-public-key>
locus adapter search linear
locus adapter install linear            # inspect envelope and confirm explicitly
locus binding add client-linear --from-adapter linear \
  --tenant client --account client-ops --credential-ref env:CLIENT_LINEAR_TOKEN \
  --scope workspace=workspace_client --read-only --non-interactive
locus adapter uninstall linear --yes
```

`install` also updates an existing adapter; there is no separate `update`
command. Installation always needs explicit consent, including tool widening.
The manifest and ledger are persisted with atomic replacement and mode 0600.
Use-time loading verifies the current publisher trust and full envelope digest
against the ledger. Bindings capture the verified envelope; runtime rechecks
current trust and exact provider/upstream equality before launch and dispatch.
Revoking a publisher key prevents subsequent cached-worker calls.

Community names cannot replace built-in provider IDs. Allowed tool names,
destructive flags and concrete scalar frozen selectors are enforced against
actual upstream calls. Extra tools are denied; `read_only` denies signed
destructive tools. Approval-required community calls remain fail-closed.
Missing or unsupported credentials prevent a resolving worker from starting.
Each community worker receives private HOME/config/temp directories for its
binding/provider slot. Exact known injected credential values are blinded in
model-facing strings and keys; trusted executable code can still transform or
send credentials, so this is not a guarantee against a malicious publisher.

The supported credential source is an explicitly supplied `env:VAR` reference.
`credential_env` must be a provider-local uppercase key ending in `API_KEY`,
`TOKEN`, `ACCESS_TOKEN` or `SECRET_KEY`, such as `LINEAR_API_KEY`. It cannot
replace HOME, PATH or Locus authority variables. `--scope KEY=VALUE` freezes a
custom signed selector as a string; dedicated flags supply built-in selectors.
Collection-valued or missing frozen selectors are refused.
Community sources also refuse Locus control/executor capabilities, session seals,
trust overlays and registry signing keys, regardless of variable-name case.
Cached workers require the same signed envelope, binding identity, principal,
tenant, policy, provider scope and credential reference used at launch.

Publishers must use **manifest_version 2**. Full-envelope signing material is
`CommunityAdapterManifest::signing_material()`: the exact typed JSON encoding
with only `entry.signature` removed, prefixed by the UTF-8 domain
`locus-community-adapter-envelope-v2` followed by NUL. Set `entry.signed_by`
before computing material and sign with
`adapter_registry::sign_entry_material_ed25519`. Ordered arrays remain JSON
arrays; no comma/pipe concatenation or legacy entry-only signatures are accepted.
The envelope covers publisher, version, credential mapping, every entry field,
and all supported upstream fields (`command`, ordered `args`, `recipe`,
`resolve_secrets`, `sandbox`, `sandbox_no_network`). Nested envelopes and unknown
runtime fields are refused. Indexes use schema 1 and a `manifest_url` for each
adapter; HTTPS and exact loopback HTTP are accepted, redirects are refused.

See [credential compatibility](./credential-compatibility.md). Public released
Locus remains 0.5.0 until the candidate passes all release gates.
