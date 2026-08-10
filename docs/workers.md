# Worker backends

Locus routes provider tools through a **worker** scoped to the active Binding × Provider.

## Backends

| Backend | Behavior |
|---------|----------|
| **Synthetic** (default) | In-process adapter tools (`supabase.scope`, freeze, policy). No child process. |
| **MCP stdio** | Spawns an upstream MCP server with isolated env, handshakes (`initialize` → `tools/list`), fans out `tools/call` over NDJSON JSON-RPC. |
| **Composite** | Per-provider routing from binding TOML: `upstream` ⇒ MCP stdio (`spawn=true`); else synthetic. |

## Isolation for MCP children

When spawning:

1. Ambient identity env is scrubbed (`AWS_PROFILE`, `GH_TOKEN`, …).
2. Only the pinned binding’s `LOCUS_*` surface is injected.
3. Optional `resolve_secrets` pulls `phm:` / `env:` into the child only.
4. Private `GH_CONFIG_DIR` / AWS config paths under the session worker home.
5. Optional **sandbox** (below) when `LOCUS_WORKER_SANDBOX=1` or `upstream.sandbox = true`.

## Worker sandbox (fail closed)

Blast radius for a malicious upstream is still **one binding’s credentials**. Sandbox is additive isolation, not a multi-tenant VM. When enabled, it means a supported OS sandbox was applied; missing backends or executables stop the spawn.

Enable globally:

```bash
export LOCUS_WORKER_SANDBOX=1
```

Or per-provider in binding TOML:

```toml
upstream = { recipe = "github-mcp", resolve_secrets = true, sandbox = true }
```

When enabled, on spawn Locus:

| Step | Behavior |
|------|----------|
| Backend | Requires macOS `/usr/bin/sandbox-exec`; unsupported platforms fail closed |
| Files | Deny by default; allow the work tree, current session worker home, system runtime files, the narrow canonical executable package/parent tree, and exact shebang interpreter |
| Authority | Denies the rest of the actual custom/default `LOCUS_HOME`, including `daemon.key`, bindings, sessions, approvals, and audit |
| Secrets | Rebuilds env from the isolation allowlist and uses a private temp root under the worker home |
| Network | Allows outbound provider traffic; denies inbound listeners |
| Provenance | Resolves the requested executable to a canonical absolute path before PATH is restricted; unavailable commands fail before spawn |
| Marker | Sets `LOCUS_WORKER_SANDBOXED=1` and `LOCUS_WORKER_SANDBOX_BACKEND=sandbox-exec` only after backend resolution succeeds |

Composite uses the same flag path: `mcp_config_from_upstream` sets `McpStdioConfig.sandbox` from the spec **or** env.

This is **not** a VM boundary. macOS Seatbelt is the only implemented backend; Linux and Windows sandbox requests currently fail closed. The explicitly allowed work tree may itself contain sensitive files, so bindings should use a narrowly scoped working directory.

## Binding TOML — per-provider upstream

```toml
[[binding.providers]]
provider = "github"
account = "acme"
credential_ref = "phm:GH_TOKEN_ACME"
scope = { orgs = ["acme-corp"] }
# Built-in recipe (recommended) — expands to command/args + hardened defaults:
upstream = { recipe = "github-official", resolve_secrets = true, sandbox = true }
# Legacy npx community package still works: recipe = "github-mcp"
# Supabase: recipe = "supabase-mcp" · Vercel remote bridge: recipe = "vercel-mcp"

# Explicit command/args (still supported):
# upstream = { command = "npx", args = ["-y", "@modelcontextprotocol/server-github"], resolve_secrets = true }

# Nested table form (applies to the most recent [[binding.providers]] entry):
# [binding.providers.upstream]
# command = "python3"
# args = ["-u", "server.py"]
```

### Built-in recipes

| Recipe | Typical use | Defaults |
|--------|-------------|-------------------------------|
| `github-official` | **Preferred** — official Docker image + `GITHUB_PERSONAL_ACCESS_TOKEN` | `resolve_secrets`, `sandbox` |
| `github-mcp` | Legacy community `@modelcontextprotocol/server-github` via `npx` (deprecated package) | `resolve_secrets`, `sandbox` |
| `supabase-mcp` | `@supabase/mcp-server-supabase` stdio (`--read-only`) | `resolve_secrets`, `sandbox` |
| `vercel-mcp` | Official remote `https://mcp.vercel.com` via documented `mcp-remote` bridge (OAuth) | `sandbox` only |
| `filesystem-mcp` | Safe filesystem demo (override root path via `args`) | off |
| `everything-mcp` | MCP test/echo server for wiring checks | off |

```bash
locus upstream list
locus upstream suggest github
locus upstream suggest supabase
locus upstream suggest vercel
```

Recipe table source: [`adapters/recipes.toml`](../adapters/recipes.toml). Explicit `command` / `args` replace the recipe's command or arguments, but do not disable its sandbox policy. Omitted `sandbox` always adopts `default_sandbox`, including command-only and args-only overrides; explicit `sandbox = false` is the only recipe opt-out. `sandbox = true` requires OS isolation, and `LOCUS_WORKER_SANDBOX=1` always forces it.

**Remote URLs (host MCP, not Locus workers):** Supabase `https://mcp.supabase.com/mcp` (optional `?project_ref=…`); Vercel `https://mcp.vercel.com` (OAuth). Locus upstream workers are **stdio** only today — use host-native remote MCP when the client supports it, or the `vercel-mcp` bridge when you need a stdio child.

When `locus-mcp` is pinned:

1. `tools/list` / `tools/call` call `CompositeWorkerManager::ensure_binding`.
2. Providers with `upstream` spawn MCP children; others stay synthetic.
3. Upstream tools are namespaced as `provider.toolname` (e.g. `github.list_issues`).
4. Synthetic tools (e.g. `github.scope`) stay available; name collisions prefer synthetic.

Worker children start from a positive runtime environment allowlist. Locus then
adds frozen binding metadata and only the provider credential keys resolved for
that binding. `McpStdioConfig::extra_env` remains for source compatibility but
is intentionally ignored; arbitrary caller or parent environment cannot cross
the worker boundary.

## API sketch

```rust
use locus_core::{
    CompositeWorkerManager, InMemoryWorkerManager, McpStdioBackend, McpStdioConfig,
    SyntheticBackend, UpstreamSpec,
};

// Default: all synthetic
let mut mgr = InMemoryWorkerManager::synthetic();
mgr.ensure_all(&session, &binding)?;

// Binding-driven composite (used by locus-mcp)
let mut mgr = CompositeWorkerManager::new();
mgr.ensure_binding(&session, &binding)?;
let tools = mgr.tools_for_pin(&session, &binding);
let r = mgr.call_tool(&session, &binding, "github.ping", &serde_json::json!({}))?;

// Manual single-backend MCP child
let backend = McpStdioBackend::new(McpStdioConfig {
    command: "npx".into(),
    args: vec!["-y".into(), "@modelcontextprotocol/server-everything".into()],
    spawn: true,
    resolve_secrets: true,
    extra_env: Default::default(),
    sandbox: false, // or true / LOCUS_WORKER_SANDBOX=1
});
let mut mgr = InMemoryWorkerManager::new(Box::new(backend));
let slot = mgr.ensure(&session, &binding, "github")?;
```

## Worker pool reuse

`CompositeWorkerManager` is process-wide inside `locus-mcp`. For a given pin:

1. `tools/list` / `tools/call` call `ensure_session` / `ensure_binding`.
2. Existing slots in `Ready` / `Running` / `Pending` are **reused** — upstream MCP children are **not** respawned per list/call.
3. Pin switch (`focus_session`) tears down slots for other `session_id`s.
4. Process exit / `Drop` tears down all remaining children.

### Optional idle timeout

```bash
export LOCUS_WORKER_IDLE_SECS=300   # tear down workers idle for 5 minutes
export LOCUS_WORKER_SANDBOX=1       # require a supported OS sandbox or fail closed
```

- Unset or `0` → never idle-reap (default).
- On `ensure_*`, the manager reaps slots whose last use exceeds the timeout, then ensures the active pin.
- `call_tool` and successful `ensure` **touch** last-used time so busy workers stay alive.
- Programmatic: `CompositeWorkerManager::with_idle_timeout`, `reap_idle`, `set_idle_timeout`.

## Capability tickets

Before provider fan-out, the multiplexor mints an HMAC capability ticket
(`session_id|binding_id|tool|exp`, 30s TTL). The audit field is `ticket_id`
(`cap_<hex>`) only — never resolved credentials. Store helpers:

```rust
let t = store.mint_capability_ticket(&session.session_id, &binding.id, "github.scope")?;
store.verify_capability_ticket(&t)?;
```

## Tests

- Unit: ensure/teardown, synthetic call, TOML `upstream` parse
- Integration: mock Python NDJSON MCP server → handshake + `tools/call`
- Composite: mixed synthetic + upstream on one binding; session focus tears down old workers
- Pool: ensure reuses same slot; `reap_idle` after timeout
- Tickets: mint/verify unit tests; store cross-key reject
- HTTP: `GET /health` unauthenticated; `POST /mcp` token reject + JSON-RPC ping
