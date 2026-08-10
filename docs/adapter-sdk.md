# Adapter SDK

How to implement a **ProviderAdapter** for Locus: freeze rules, tools, tests, and registration.

Sibling guide with binding TOML examples: [adapters.md](./adapters.md).  
Architecture: [architecture.md](./architecture.md) · full design: [DESIGN.md](../DESIGN.md).

**Invariants (do not break):**

1. Scope freeze before policy/approval — wrong tenant selectors are hard errors.
2. Credential values never appear in tool results or audit details.
3. Destructive tools are marked `destructive: true` and covered by policy globs.
4. Adapters own provider-specific knowledge only (`crates/locus-core/src/adapters/`).

---

## Layout

```
crates/locus-core/src/adapters/
  mod.rs          # ProviderAdapter, freeze_*, adapter_for, dispatch, control tools
  supabase.rs
  github.rs
  vercel.rs
  cloudflare.rs
  aws.rs
  stripe.rs
  resend.rs

crates/locus-core/src/adapter_registry.rs  # catalog parse + signature verify

adapters/manifest.toml                     # built-in provider catalog + capabilities
schema/adapter-manifest.schema.json        # JSON Schema for the catalog shape
examples/adapters/_template/               # copy-paste skeleton
```

Built-in providers and tools are listed in [`adapters/manifest.toml`](../adapters/manifest.toml).

### Registry CLI (v0)

Discovery and signature verification over the catalog — **not** a plugin loader:

```bash
locus adapter list [--json]
locus adapter verify [--path FILE] [--require-signed] [--json]
locus topic adapter
```

| Command | Behavior |
|---------|----------|
| `adapter list` | Print every provider in the embedded manifest (tools, freeze selectors, caps) |
| `adapter verify` | Soft by default: unsigned entries OK; **invalid/malformed** signatures fail |
| `adapter verify --require-signed` | **Fail closed** unless every entry has a valid trusted signature |

Sibling: `locus upstream list` covers **spawn recipes** (`adapters/recipes.toml`); `locus adapter` covers the **provider catalog**.

---

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

| `AdapterTool` field | Purpose |
|---------------------|---------|
| `name` | MCP tool name — convention `provider.action` |
| `description` | Model-facing; **include frozen scope** so the agent sees the fence |
| `input_schema` | JSON Schema for arguments |
| `provider` | Provider id string (must match binding `provider`) |
| `destructive` | UX/policy hint; still enforce with `require_approval` globs |

---

## Scope freeze (required)

Use shared helpers so preflight and adapter `call` stay consistent:

```rust
use super::{freeze_string_arg, freeze_bool_arg};

// In call() — and preflight already freezes project_ref / team_id / account_id / livemode:
let project_ref = freeze_string_arg(args, "project_ref", provider.scope.project_ref.as_deref())?;
let livemode = freeze_bool_arg(args, "livemode", Some(false))?;
```

| Helper | Use for |
|--------|---------|
| `freeze_string_arg(args, key, frozen)` | Single frozen string (`project_ref`, `team_id`, `account_id`) |
| `freeze_bool_arg(args, key, frozen)` | Booleans (`livemode`) |
| Custom allowlist checks | Orgs/repos, env targets, domains (see `github`, `vercel`, `resend`) |

**Rule:** binding freezes a selector → model mismatch is **`Err` (scope freeze)**, never a soft warn.

`call_tool_gated` runs `preflight_scope_freeze` **before** policy so a wrong `project_ref` cannot mint an approval grant for the wrong tenant.

---

## Register

1. Add `mod myprovider;` and `pub use myprovider::MyProviderAdapter;` in `adapters/mod.rs`.
2. Add a match arm in `adapter_for()`:

```rust
pub fn adapter_for(provider: &str) -> Option<Box<dyn ProviderAdapter>> {
    match provider.to_ascii_lowercase().as_str() {
        // ...
        "myprovider" => Some(Box::new(MyProviderAdapter)),
        _ => None,
    }
}
```

3. Update `adapters/manifest.toml` with tools, frozen selectors, and capabilities.
4. Add freeze + happy-path unit tests (module-local or `adapters/mod.rs` tests).

Unknown providers still get a generic `{provider}.scope` identity tool via `tools_for_binding` — fine for experiments, not for production freeze rules.

---

## Dispatch order

```
tools/call
  → preflight_scope_freeze   # hard error on selector mismatch
  → enforce_policy           # deny | require_approval | allow
  → adapter.call             # re-check freeze; build ToolCallResult
```

Tool names must be prefixed with the provider: `supabase.table.delete`.  
Routing uses the first segment before `.`.

---

## Tests (minimum)

```rust
#[test]
fn freeze_rejects_wrong_selector() {
    let b = sample_binding();
    let err = call_tool(&b, "myprovider.scope", &json!({"account_id": "evil"}));
    assert!(err.is_err());
    assert!(err.unwrap_err().to_string().contains("scope freeze"));
}

#[test]
fn happy_path_returns_frozen_scope() {
    let b = sample_binding();
    let r = call_tool(&b, "myprovider.scope", &json!({})).unwrap();
    assert!(r.ok);
    assert_eq!(r.content["account_id"], "acct_good");
}

#[test]
fn destructive_requires_approval() {
    // Policy require_approval: ["*.delete*"]
    let r = call_tool(&b, "myprovider.widget.delete", &json!({"id": "1"})).unwrap();
    assert!(!r.ok);
    assert_eq!(r.content["error"], "requires_approval");
}
```

Run:

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo test -p locus-core adapters
cargo clippy -p locus-core --all-targets -- -D warnings
```

---

## Template

Copy [`examples/adapters/_template/`](../examples/adapters/_template/) and follow its README.

Skeleton shape:

```rust
use super::{freeze_string_arg, AdapterTool, ProviderAdapter, ToolCallResult};
use crate::binding::{Binding, ProviderBinding};
use crate::error::Result;
use serde_json::{json, Value};

pub struct MyProviderAdapter;

impl ProviderAdapter for MyProviderAdapter {
    fn name(&self) -> &'static str { "myprovider" }

    fn tools(&self, provider: &ProviderBinding, binding: &Binding) -> Vec<AdapterTool> {
        let account = provider.scope.account_id.as_deref().unwrap_or("<unset>");
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
        let account_id = freeze_string_arg(args, "account_id", provider.scope.account_id.as_deref())?;
        match tool {
            "myprovider.scope" => Ok(ToolCallResult {
                ok: true,
                content: json!({
                    "provider": "myprovider",
                    "account": provider.account,
                    "account_id": account_id.or_else(|| provider.scope.account_id.clone()),
                    "credential_ref": provider.credential_ref,
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

---

## What not to do

- Return secret values or resolved tokens in `ToolCallResult.content`
- Fall through to ambient `gh auth`, global AWS profile, or another binding’s env
- Call remote APIs with credentials outside the pinned binding’s CredentialRefs
- Log to stdout from MCP-adjacent code (stdout is JSON-RPC only)
- Soft-allow scope mismatches or invent alternate `project_ref` / teams

---

## Checklist before merge

- [ ] `freeze_*` (or allowlist) for every hard scope knob  
- [ ] Freeze unit tests (deny + happy path)  
- [ ] Destructive tools: `destructive: true` + policy coverage  
- [ ] No secrets in tool responses  
- [ ] Registered in `adapter_for` + `adapters/manifest.toml`  
- [ ] `cargo test -p locus-core` + `cargo clippy -p locus-core -- -D warnings`  
- [ ] Row in this doc / [adapters.md](./adapters.md) if user-facing  

---

## Phase 1 vs later

| Now | Later |
|-----|--------|
| In-tree `ProviderAdapter`, synthetic identity tools | Upstream MCP / REST workers (partially landed via recipes) |
| Manual `adapter_for` match | Optional out-of-tree packages / dynamic load |
| Catalog parse + HMAC signature verify (`locus adapter`) | Signed registry root (ed25519) + published trust anchors |
| Policy + approval CLI | Elevation TTL, dual-control UX polish |

Prefer **wrapping** official upstream MCP servers with frozen env over reimplementing APIs — see [workers.md](./workers.md).

---

## Signed registry roadmap

The adapter **catalog** (`adapters/manifest.toml`) is the first registry primitive. v0 ships parse + list + verify; it does **not** load plugins or replace `adapter_for()`.

### Manifest fields (v0)

```toml
version = 1

[[providers]]
id = "github"
name = "GitHub"
status = "built-in"
synthetic = true
capabilities = ["scope", "identity", "repo_allowlist"]
frozen_selectors = ["orgs", "repos"]
tools = ["github.scope", "github.whoami", "github.check_repo"]
destructive_tools = []
description = "…"
# Optional detached signature (omit until a registry root is published):
# signature = "hmac-sha256:<64-hex>"
# signed_by = "locus-registry-mock"
```

JSON Schema: [`schema/adapter-manifest.schema.json`](../schema/adapter-manifest.schema.json).

### Signature scheme (v0)

| Item | v0 (now) | Later |
|------|----------|--------|
| Algorithm | HMAC-SHA256 over canonical entry material | ed25519 detached sig |
| Wire form | `hmac-sha256:<hex>` | `ed25519:<base64>` |
| Trust store | Process-local keys (tests inject a mock key; production default = empty) | Published registry root key(s) under `~/.locus/trust/` + offline pin |
| Built-in catalog | **Unsigned** (soft verify passes; `--require-signed` fails closed) | Signed at release with the registry root |
| Scope | Per-provider entry | + whole-file signature, transparency log optional |

**Canonical material** (stable, excludes signature fields):

```text
v1|{id}|{name}|{status}|{synthetic 0|1}|{caps sorted}|{selectors sorted}|{tools sorted}|{destructive sorted}
```

### Verify semantics

| Mode | Unsigned | Unknown key | Invalid / malformed |
|------|----------|-------------|---------------------|
| Soft (`locus adapter verify`) | OK (informational) | OK (informational) | **Fail** (tamper) |
| Strict (`--require-signed`) | **Fail** | **Fail** | **Fail** |

`--require-signed` is intentionally fail-closed so CI / firm policy can require a trusted catalog before enabling experimental adapters.

### What is intentionally out of scope (v0)

- Dynamic `.so` / WASM / script adapter loading  
- Network fetch of remote registries  
- Replacing in-tree `adapter_for()` registration  
- Returning or storing credential values in the catalog  

### Next slices

1. **Publish a registry root** (ed25519) and sign release manifests in CI.  
2. **Trust pin UX** — `locus adapter trust add|list` writing under `~/.locus/trust/`.  
3. **Optional remote index** — signed HTTP feed of community adapters (still no auto-exec).  
4. **Out-of-tree packages** — crate or MCP child declared in the catalog, still gated by pin + policy + freeze.  
