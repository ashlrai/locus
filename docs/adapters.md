# Writing a provider adapter

> **SDK guide:** [adapter-sdk.md](./adapter-sdk.md) · **template:** [examples/adapters/_template/](../examples/adapters/_template/) · **catalog:** [adapters/manifest.toml](../adapters/manifest.toml)

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
credential_ref = "phm:SUPABASE_ACME"
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
