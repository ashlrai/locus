# Writing a provider adapter

> **SDK guide:** [adapter-sdk.md](./adapter-sdk.md) · **template:** [examples/adapters/_template/](../examples/adapters/_template/) · **catalog:** [adapters/manifest.toml](../adapters/manifest.toml) · **schema:** [schema/adapter-manifest.schema.json](../schema/adapter-manifest.schema.json) · **CLI:** `locus adapter list` / `locus adapter verify`

Adapters are the **only** place provider-specific knowledge should live in Locus. They define:

1. Which **tools** appear when a binding includes that provider  
2. How **scope freeze** rejects model-supplied account selectors  
3. How tool calls are answered: synthetic identity/scope tools in-process, or an upstream MCP server spawned as an isolated worker (see [workers.md](./workers.md))

Canonical design: [DESIGN.md §8](../DESIGN.md) (adapter model) and §9 (threat model).

## Where adapters live (today)

Built-in adapters are in-tree Rust modules:

```
crates/locus-core/src/adapters/
  mod.rs          # ProviderAdapter trait, freeze helpers, dispatch, control tools
  supabase.rs
  github.rs
  vercel.rs
  cloudflare.rs
  aws.rs
  resend.rs
  stripe.rs
  anthropic.rs
  openai.rs
  cli.rs          # native CLI routing helpers (not a provider adapter)
  model_api_tests.rs  # shared Anthropic/OpenAI freeze tests (cfg(test))
```

Registration is a match arm in `adapter_for()`:

```rust
pub fn adapter_for(provider: &str) -> Option<Box<dyn ProviderAdapter>> {
    match provider.to_ascii_lowercase().as_str() {
        "supabase" => Some(Box::new(SupabaseAdapter)),
        "github" => Some(Box::new(GithubAdapter)),
        "vercel" => Some(Box::new(VercelAdapter)),
        "cloudflare" => Some(Box::new(CloudflareAdapter)),
        "aws" => Some(Box::new(AwsAdapter)),
        "resend" => Some(Box::new(ResendAdapter)),
        "stripe" => Some(Box::new(StripeAdapter)),
        "anthropic" => Some(Box::new(AnthropicAdapter)),
        "openai" => Some(Box::new(OpenaiAdapter)),
        _ => None,
    }
}
```

`known_providers()` lists the same names and a test keeps it in sync with `adapter_for()`. Unknown providers still get a generic `{provider}.scope` identity tool via `tools_for_binding`, which is useful for experimentation, but a real adapter should own freeze rules.

## The trait

```rust
pub trait ProviderAdapter: Send + Sync {
    fn name(&self) -> &'static str;

    // Optional native CLI routing (defaults shown; see cli-routing.md).
    fn cli_name(&self) -> Option<&'static str> { None }
    fn cli_required_credentials(&self) -> &'static [&'static str] {
        crate::credential::inject_keys_for_provider(self.name())
    }
    fn configure_cli_env(
        &self,
        _provider: &ProviderBinding,
        _vars: &mut BTreeMap<String, String>,
    ) {}

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

Only `name`, `tools` and `call` are required. Override `cli_name` (e.g. GitHub returns `gh`, AWS returns `aws`) to allow single-provider native CLI routing; `cli_required_credentials` lists the keys that must be nonempty before a routed CLI starts, and `configure_cli_env` adds provider defaults (AWS disables instance metadata and sets the frozen region). See [cli-routing.md](./cli-routing.md).

`AdapterTool` fields:

| Field | Purpose |
|-------|---------|
| `name` | MCP tool name — convention `provider.action` (e.g. `supabase.scope`) |
| `description` | Model-facing; **include frozen scope** so the agent sees the fence |
| `input_schema` | JSON Schema object for arguments |
| `provider` | Provider id string |
| `destructive` | Hidden from tools/list and denied at the call gate when the provider scope is `read_only = true`; also gate it with `require_approval` globs |

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
| GitHub | `orgs[]`, `repos[]` allowlists (empty = unrestricted; `*` and `owner/*` wildcards) |
| Vercel | `team_id` (also the `team` key), `env` targets (preview/prod); `projects` reported |
| Cloudflare | `account_id`; `zones` (from `projects` or `extra.zones`) reported |
| AWS | `account_id`, plus `profile` and `region` from scope `extra`; `role_arn` reported |
| Resend | `domain` must be in the `domains` allowlist (`projects` or `extra.domains`) |
| Stripe | `account_id`, `livemode` (boolean, from scope `extra`) |
| Anthropic | `account_id` (org id), `project_ref` (workspace id), `read_only` — per-tenant model-API spend isolation; aliases `org`/`org_id`/`organization`/`workspace`/`workspace_id` (+ camelCase) are freeze-netted |
| OpenAI | `account_id` (org id), `project_ref` (project id), `read_only` — per-tenant model-API spend isolation; aliases `org`/`org_id`/`organization`/`project`/`project_id` (+ camelCase) are freeze-netted |

**Rule:** if the binding freezes a selector, model-supplied mismatch → **error**, not warn.

Before any adapter runs, the gate also freezes `project_ref`, `team_id` and `account_id` (plus provider-native and camelCase alias spellings, at any depth in the args), the org allowlist and `livemode` for every provider. Re-check your selectors in `call()` anyway.

## Tool naming and dispatch

- Tools must be prefixed with the provider name: `supabase.table.delete`.
- `call_tool` in `mod.rs` routes by the first segment before `.`; a prefix that is not a provider in the pinned binding is denied.
- The gate runs **before** adapter `call`, in this order: scope freeze, closed schema (`additionalProperties: false` rejects undeclared args; `confirm` and `approval_id` are always accepted), `read_only` scope vs `destructive`, session budget, then policy (deny / require_approval / allow). See [policy.md](./policy.md).
- `require_approval` (e.g. `*.delete*`) blocks the call and records a pending approval. Local `locus approve grant` records are advisory, and no external approval verifier ships yet, so approval-gated provider calls stay blocked even with `confirm: true`.

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
5. **Register** in `adapter_for`, `known_providers()` (the `known_providers_all_dispatch` test keeps them in sync) and `mod myprovider`.
6. **Tests** in `adapters/mod.rs` or the new module:
   - freeze rejects wrong selector
   - happy path returns frozen scope
   - a destructive tool is denied under `read_only = true` and blocked by a `require_approval` glob
7. **Docs**: one row in the matrix below; binding example if user-facing.
8. **Credential confinement**: provider credentials may resolve into isolated `locus exec`, `locus run`, and `locus ci run` children by default. Use their shared `--no-resolve` mode for identity-only diagnostics; it rejects recipe-expanded resolving upstreams before effects. CI `mint/env --resolve` additionally requires `LOCUS_CI_ALLOW_SECRETS=1`. Never return credentials through MCP results or logs.

### Minimal skeleton

```rust
use super::{freeze_string_arg, AdapterTool, ProviderAdapter, ToolCallResult};
use crate::binding::{Binding, ProviderBinding};
use crate::error::Result;
use serde_json::{json, Value};

pub struct MyproviderAdapter;

impl ProviderAdapter for MyproviderAdapter {
    fn name(&self) -> &'static str {
        "myprovider"
    }

    fn tools(&self, provider: &ProviderBinding, binding: &Binding) -> Vec<AdapterTool> {
        let account = provider
            .scope
            .account_id
            .as_deref()
            .unwrap_or("<unset>");
        vec![AdapterTool {
            name: "myprovider.scope".into(),
            description: format!(
                "Frozen myprovider scope for `{}` / `{}`: account_id={account}",
                binding.tenant, binding.alias
            ),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "account_id": { "type": "string" }
                },
                "additionalProperties": false
            }),
            provider: "myprovider".into(),
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
            "myprovider.scope" => Ok(ToolCallResult {
                ok: true,
                content: json!({
                    "provider": "myprovider",
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

| Now | Later (roadmap) |
|-----|-----------------|
| Synthetic tools with local freeze; upstream MCP stdio workers from recipes (`adapters/recipes.toml`, `locus upstream list`, [workers.md](./workers.md)) | REST workers |
| In-tree `ProviderAdapter`; catalog parse + verify (`locus adapter`); signed community adapters as isolated MCP workers ([below](#community-adapter-marketplace)) | Dynamic in-process adapter loading |
| Policy `allow` / `deny` / `require_approval` / `dual_control`, approval CLI and session TTL ([policy.md](./policy.md)) | Elevation TTL |

Prefer **wrapping** official upstream MCP servers with frozen env over reimplementing APIs — see DESIGN §8.3 and [workers.md](./workers.md).

## Checklist before merge

- [ ] Freeze tests for every hard scope knob  
- [ ] No secrets in tool responses  
- [ ] Destructive tools covered by policy globs or explicit gates  
- [ ] `cargo test -p locus-core` green  
- [ ] `cargo clippy -p locus-core -- -D warnings` clean  

Credential resolution currently supports explicitly supplied `env:VAR` references. Stored `phm:` references remain valid metadata but require an unavailable scoped bridge; never implement an adapter by capturing trusted-terminal reveal. See [credential compatibility](./credential-compatibility.md).

## Community adapter marketplace

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

Community adapters are new in 0.5.1.
