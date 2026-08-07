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

## Binding TOML — per-provider upstream

```toml
[[binding.providers]]
provider = "github"
account = "acme"
credential_ref = "phm:GH_TOKEN_ACME"
scope = { orgs = ["acme-corp"] }
upstream = { command = "npx", args = ["-y", "@modelcontextprotocol/server-github"], resolve_secrets = true }

# Nested table form (applies to the most recent [[binding.providers]] entry):
# [binding.providers.upstream]
# command = "python3"
# args = ["-u", "server.py"]
```

When `locus-mcp` is pinned:

1. `tools/list` / `tools/call` call `CompositeWorkerManager::ensure_binding`.
2. Providers with `upstream` spawn MCP children; others stay synthetic.
3. Upstream tools are namespaced as `provider.toolname` (e.g. `github.list_issues`).
4. Synthetic tools (e.g. `github.scope`) stay available; name collisions prefer synthetic.

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
});
let mut mgr = InMemoryWorkerManager::new(Box::new(backend));
let slot = mgr.ensure(&session, &binding, "github")?;
```

## Tests

- Unit: ensure/teardown, synthetic call, TOML `upstream` parse
- Integration: mock Python NDJSON MCP server → handshake + `tools/call`
- Composite: mixed synthetic + upstream on one binding; session focus tears down old workers
