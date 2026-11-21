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
  mod.rs          # ProviderAdapter, freeze_*, adapter_for, known_providers, dispatch, control tools
  cli.rs          # CliRoute: native CLI credential routing for `locus exec --for`
  supabase.rs
  github.rs
  vercel.rs
  cloudflare.rs
  aws.rs
  stripe.rs
  resend.rs
  anthropic.rs
  openai.rs

crates/locus-core/src/adapter_registry.rs  # catalog parse + signature verify

adapters/manifest.toml                     # built-in provider catalog + capabilities
schema/adapter-manifest.schema.json        # JSON Schema for the catalog shape
examples/adapters/_template/               # copy-paste skeleton
```

Built-in providers and tools are listed in [`adapters/manifest.toml`](../adapters/manifest.toml).

### Registry CLI (v0)

Discovery and signature verification over the built-in catalog. These commands never load code:

```bash
locus adapter list [--json]
locus adapter verify [--path FILE] [--require-signed] [--json]
locus adapter trust list [--json]
locus adapter trust add --id root --ed25519-pub <b64> [--json]
locus topic adapter
```

| Command | Behavior |
|---------|----------|
| `adapter list` | Print every provider in the embedded manifest (tools, freeze selectors, caps) |
| `adapter verify` | Soft by default: unsigned entries OK; **invalid/malformed** signatures fail. Uses **merged** trust keys (file + env). |
| `adapter verify --require-signed` | **Fail closed** unless every entry has a valid trusted signature |
| `adapter trust list` | List keys from `$LOCUS_HOME/trust/adapter-keys.toml` + `LOCUS_ADAPTER_TRUST_KEYS` overlay |
| `adapter trust add` | Pin an ed25519 public key under the file store (mode `0600`) |

Release manifests of the built-in set: `locus adapter registry export [--out FILE] [--sign [--key FILE] [--key-id ID]]` and `locus adapter verify-manifest FILE [--allow-unsigned]` (see [registry-trust.md](./registry-trust.md)).

Community adapters are separate: `locus adapter search|install|uninstall` and `locus adapter registry index add|list|remove` fetch signed full envelopes for out-of-tree MCP stdio workers, which run publisher code. See [adapters.md § Community adapter marketplace](./adapters.md#community-adapter-marketplace). This guide covers in-tree adapters only.

Sibling: `locus upstream list` covers **spawn recipes** (`adapters/recipes.toml`); `locus adapter` covers the **provider catalog**.

---

## The trait

```rust
pub trait ProviderAdapter: Send + Sync {
    fn name(&self) -> &'static str;

    // Optional, with defaults: native CLI routing for `locus exec --for <cli>`.
    fn cli_name(&self) -> Option<&'static str> { None }
    fn cli_required_credentials(&self) -> &'static [&'static str] { /* inject keys for name() */ }
    fn configure_cli_env(&self, _provider: &ProviderBinding, _vars: &mut BTreeMap<String, String>) {}

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
| `input_schema` | JSON Schema for arguments. With `"additionalProperties": false` the gate denies undeclared arguments before policy (`confirm` and `approval_id` are always accepted) |
| `provider` | Provider id string (must match binding `provider`) |
| `destructive` | Hidden from `tools/list` and denied at call time (`denied_read_only_scope`) when the provider scope is `read_only = true`; still cover with `require_approval` globs |

Only `gh`, `vercel`, `supabase` and `aws` override `cli_name` today. Leave the defaults unless the provider has a native CLI that should receive the pinned credentials.

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

`call_tool_gated` runs `preflight_scope_freeze` **before** policy so a wrong `project_ref` cannot mint an approval grant for the wrong tenant. The preflight also denies a tool whose provider prefix is not in the pinned binding, and checks `project_ref` / `team_id` / `account_id`, `orgs` and `livemode` under their camelCase and provider-native spellings at any nesting depth. Adapters still re-check their own selectors in `call()`.

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

3. Add the id to `known_providers()` in `adapters/mod.rs`.
4. Update `adapters/manifest.toml` with tools, frozen selectors, and capabilities.
5. Add freeze + happy-path unit tests (module-local or `adapters/mod.rs` tests).

Two drift tests fail if you miss a step: `known_providers_all_dispatch` (every `known_providers()` id dispatches through `adapter_for`) and `builtin_catalog_matches_dispatchable_providers` (`known_providers()` and `manifest.toml` list the same ids).

Unknown providers still get a generic `{provider}.scope` identity tool via `tools_for_binding`. It returns credential metadata only, and any other tool for that provider errors with `no adapter for provider`. Fine for experiments, not for production freeze rules.

---

## Dispatch order

```
tools/call → call_tool_gated → enforce_policy:
  → preflight_scope_freeze   # hard error: provider not in binding, or selector mismatch
  → reject_undeclared_args   # hard error: argument outside a closed input_schema
  → enforce_read_only_scope  # denied_read_only_scope for destructive tools under read_only
  → enforce_session_policy   # opt-in [budget] charge, then deny | require_approval | allow
→ adapter.call               # re-check freeze; build ToolCallResult
```

No external approval verifier ships today, so a `require_approval` tool always returns `requires_approval` with an advisory `approval_id`; see [policy.md](./policy.md).

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
                    // { present, source } only; the locator name never crosses.
                    "credential": crate::credential::credential_metadata(&provider.credential_ref),
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
- Return the raw `credential_ref` string (use `credential_metadata`, as the built-in adapters do)
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
- [ ] Registered in `adapter_for` + `known_providers()` + `adapters/manifest.toml`  
- [ ] `cargo test -p locus-core` + `cargo clippy -p locus-core -- -D warnings`  
- [ ] Row in this doc / [adapters.md](./adapters.md) if user-facing  

---

## Phase 1 vs later

| Now | Later |
|-----|--------|
| In-tree `ProviderAdapter`, synthetic identity tools | Upstream MCP / REST workers (partially landed via recipes) |
| Manual `adapter_for` match; community adapters run as signed out-of-tree MCP stdio workers ([adapters.md](./adapters.md#community-adapter-marketplace)) | Dynamic in-process load |
| Catalog parse + **ed25519** (preferred) / HMAC-SHA256 (backcompat) verify (`locus adapter`) | Registry root publication |
| Signed release manifests (`locus adapter registry export --sign` / `verify-manifest`, see [registry-trust.md](./registry-trust.md)) | CI-side signing (needs a provisioned `LOCUS_REGISTRY_SIGNING_KEY` secret) |
| Policy + approval CLI | Elevation TTL, dual-control UX polish |

Prefer **wrapping** official upstream MCP servers with frozen env over reimplementing APIs — see [workers.md](./workers.md).

---

## Signed registry roadmap

The adapter **catalog** (`adapters/manifest.toml`) is the first registry primitive. It ships parse + list + multi-scheme verify; it does **not** load plugins or replace `adapter_for()`.

### Manifest fields

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
# signature = "ed25519:<base64>"          # preferred
# signature = "hmac-sha256:<64-hex>"      # backcompat
# signed_by = "locus-registry-root"
```

JSON Schema: [`schema/adapter-manifest.schema.json`](../schema/adapter-manifest.schema.json).

### Signature schemes

| Item | Preferred | Backcompat |
|------|-----------|------------|
| Algorithm | ed25519 detached over canonical entry material | HMAC-SHA256 over the same material |
| Wire form | `ed25519:<base64>` (64-byte sig) | `hmac-sha256:<hex>` (32-byte MAC) |
| Trust material | 32-byte public key, standard base64 | 32-byte secret, 64 hex chars |
| Trust load | `$LOCUS_HOME/trust/adapter-keys.toml` + `LOCUS_ADAPTER_TRUST_KEYS` (env overlays file) / test fixtures (`verify_*_with_keys`) | same |
| Built-in catalog | **Unsigned** (soft verify passes; `--require-signed` fails closed) until CI signs with the registry root | same |

`locus adapter verify --require-signed` accepts **either** scheme when the `signed_by` key id is present and trusted and the signature verifies.

**Canonical material** (stable, excludes signature fields):

```text
v1|{id}|{name}|{status}|{synthetic 0|1}|{caps sorted}|{selectors sorted}|{tools sorted}|{destructive sorted}
```

### Production trust store (`$LOCUS_HOME/trust/`)

File path: **`$LOCUS_HOME/trust/adapter-keys.toml`** (directory mode `0700`, file mode `0600`).

```toml
version = 1

[[keys]]
id = "root"
scheme = "ed25519"
public_key_b64 = "<standard-base64 of 32-byte verifying key>"

# Optional HMAC backcompat (prefer ed25519 in production):
# [[keys]]
# id = "mock"
# scheme = "hmac-sha256"
# secret_hex = "<64 hex chars>"
```

```bash
# Pin a registry root (writes adapter-keys.toml with mode 0600)
locus adapter trust add --id root --ed25519-pub '<base64-pubkey>'
locus adapter trust list

# Optional env overlay (wins on same id — useful for CI)
export LOCUS_ADAPTER_TRUST_KEYS="root:ed25519:<base64-pubkey>,mock:hmac-sha256:<64-hex-secret>"

locus adapter verify --require-signed
```

**Merge order:** load file keys, then overlay `LOCUS_ADAPTER_TRUST_KEYS` (same id → env replaces). Production ships **no** baked-in keys. An empty / unset trust store keeps soft verify OK for the unsigned built-in catalog; strict mode fails closed.

### Verify semantics

| Mode | Unsigned | Unknown key | Invalid / malformed |
|------|----------|-------------|---------------------|
| Soft (`locus adapter verify`) | OK (informational) | OK (informational) | **Fail** (tamper) |
| Strict (`--require-signed`) | **Fail** | **Fail** | **Fail** |

`--require-signed` is intentionally fail-closed so CI / firm policy can require a trusted catalog before enabling experimental adapters.

### What is intentionally out of scope (now)

- Dynamic `.so` / WASM / script adapter loading  
- Network fetch of the built-in catalog (community indexes are a separate path)  
- Replacing in-tree `adapter_for()` registration  
- Returning or storing credential values in the catalog  
- Baked-in production root keys (must be operator-supplied)

### Production trust store (status)

| Item | Status |
|------|--------|
| File store `$LOCUS_HOME/trust/adapter-keys.toml` | **Landed** |
| `locus adapter trust add|list` | **Landed** |
| Merge with `LOCUS_ADAPTER_TRUST_KEYS` | **Landed** (env overlays file) |
| Publish registry root + sign release manifests in CI | Roadmap |
| Offline pin rotation / revoke / expiry | Roadmap |
| Remote community index (untrusted discovery, signed full envelopes) | **Landed** ([adapters.md](./adapters.md#community-adapter-marketplace)) |
| Out-of-tree packages | **Landed** as community MCP stdio workers; no in-process loading |
| Whole-file signature + transparency log | Roadmap |

1. **Publish a registry root** (ed25519) and sign release manifests in CI; ship the public key as a well-known pin, not a private secret.  
2. **Offline pin + rotation** — multiple concurrent roots, expiry, and explicit revoke without soft-allow on unknown keys under `--require-signed`.  
3. **Remote index** — landed for community adapters: HTTPS index of signed envelopes, explicit install consent, no auto-exec.  
4. **Out-of-tree packages** — landed as community MCP children, gated by pin + policy + freeze; in-process crates remain out of scope.  
5. **Whole-file signature** + optional transparency log once per-entry roots are stable.  
