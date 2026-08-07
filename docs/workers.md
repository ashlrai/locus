# Worker backends

Locus routes provider tools through a **worker** scoped to the active Binding × Provider.

## Backends

| Backend | Behavior |
|---------|----------|
| **Synthetic** (default) | In-process adapter tools (`supabase.scope`, freeze, policy). No child process. |
| **MCP stdio** | Spawns an upstream MCP server with isolated env, handshakes (`initialize` → `tools/list`), fans out `tools/call` over NDJSON JSON-RPC. |

## Isolation for MCP children

When spawning:

1. Ambient identity env is scrubbed (`AWS_PROFILE`, `GH_TOKEN`, …).
2. Only the pinned binding’s `LOCUS_*` surface is injected.
3. Optional `resolve_secrets` pulls `phm:` / `env:` into the child only.
4. Private `GH_CONFIG_DIR` / AWS config paths under the session worker home.

## API sketch

```rust
use locus_core::{InMemoryWorkerManager, McpStdioBackend, McpStdioConfig, SyntheticBackend};

// Default: synthetic
let mut mgr = InMemoryWorkerManager::synthetic();
mgr.ensure_all(&session, &binding)?;

// Upstream MCP child
let backend = McpStdioBackend::new(McpStdioConfig {
    command: "npx".into(),
    args: vec!["-y".into(), "@modelcontextprotocol/server-everything".into()],
    spawn: true,
    resolve_secrets: true,
    extra_env: Default::default(),
});
let mut mgr = InMemoryWorkerManager::new(Box::new(backend));
let slot = mgr.ensure(&session, &binding, "github")?;
// tools/call routed via WorkerBackend::call_tool
```

## Config (coming)

Per-provider upstream will be expressible in binding TOML:

```toml
[[binding.providers]]
provider = "github"
account = "acme"
credential_ref = "phm:GH_TOKEN_ACME"
scope = { orgs = ["acme-corp"] }
# future:
# upstream = { command = "npx", args = ["-y", "@github/mcp"] }
```

Until then, construct `McpStdioConfig` in code or set `LOCUS_MCP_UPSTREAM_*` env (CLI wiring TBD).

## Tests

- Unit: ensure/teardown, synthetic call
- Integration: mock Python NDJSON MCP server → handshake + `tools/call`
