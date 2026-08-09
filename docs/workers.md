# Worker backends

Locus routes provider tools through a **worker** scoped to the active Binding × Provider.

## Backends

| Backend | Behavior |
|---------|----------|
| **Synthetic** (default) | In-process adapter tools (`supabase.scope`, freeze, policy). No child process. |
| **MCP stdio** | Experimental host child, disabled by default. With explicit unsafe opt-in, handshakes and fans out manifest-admitted `tools/call` requests over NDJSON JSON-RPC. |
| **Composite** | Per-provider routing from binding TOML: `upstream` ⇒ MCP stdio (`spawn=true`); else synthetic. |

## Security boundary for MCP children

Locus does **not** currently sandbox upstream processes. They run as the same OS
user and can read user-readable files, including `LOCUS_HOME/daemon.key`. File
mode `0600` does not separate processes with the same uid. An upstream can also
use its injected credential directly over the network, outside the MCP response
path.

For that reason, host execution fails closed unless the binding explicitly sets
`unsafe_host_execution = true`. This flag is an unsafe development opt-in, not
a confinement mode. Keep it false for untrusted upstream packages and
production-capable credentials. Synthetic adapters remain available when spawn
is denied.

After opt-in, the MCP boundary applies these additional controls.

When spawning:

1. The child starts with an empty environment. Only process mechanics needed to
   launch common runtimes are copied: `PATH`, temporary-directory variables,
   locale/timezone variables, and required Windows process variables.
2. `HOME`, `USERPROFILE`, and `PWD` point at the selected worker directory.
   Package-manager, proxy, cloud, vault, and arbitrary parent variables are not
   inherited.
3. Every binding provider's `env:` target name and standard credential injection
   keys are removed before credentials are added. This includes non-`LOCUS_`
   locator names.
4. Only the selected provider's account, CredentialRef, known scope fields, and
   provider-specific config paths/aliases are exported. No provider catalog or
   other provider metadata is exposed.
5. Optional `resolve_secrets` injects only the selected provider's credential
   under that provider's standard injection keys. The source `env:` locator is
   not inherited by name.

Programmatic `McpStdioConfig.extra_env` entries are limited to overrides of the
same runtime allowlist; arbitrary provider or credential variables are ignored.

When `resolve_secrets = true`, a resolution failure denies spawn rather than
starting a child that might fall back to same-user credential files.

Before each call, a closed capability manifest admits the tool and classifies
every top-level argument. `passthrough` is an explicit assertion that the
argument has no selector semantics. Selector arguments bind to `account`,
`scope.project_ref`, `scope.team_id`, `scope.account_id`, `scope.orgs`,
`scope.repos`, `scope.projects`, or `scope.env`. Unknown tools, arguments, and
semantics deny. A single frozen selector is injected when omitted; list-valued
scope requires an exact declared value when more than one is allowed.

Before returning, Locus scans upstream tool metadata, results, and errors for
credential values injected into that child. A match discards the upstream
payload and returns a generic error. This is defense at the MCP return boundary;
it does not contain the worker's filesystem or network access.

## Binding TOML — per-provider upstream

```toml
[[binding.providers]]
provider = "github"
account = "acme"
credential_ref = "phm:GH_TOKEN_ACME"
scope = { orgs = ["acme-corp"] }
[binding.providers.upstream]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
resolve_secrets = true
unsafe_host_execution = false # change to true only for explicit unsafe development use

[binding.providers.upstream.capabilities.list_issues.arguments]
owner = "scope.orgs"
state = "passthrough"

[binding.providers.upstream.capabilities.get_issue.arguments]
owner = "scope.orgs"
repo = "scope.repos"
issue_number = "passthrough"
```

When `locus-mcp` is pinned:

1. `tools/list` / `tools/call` call `CompositeWorkerManager::ensure_binding`.
2. Providers with admitted `upstream` configuration spawn only after explicit unsafe host opt-in; others stay synthetic.
3. Manifest-admitted upstream tools are namespaced as `provider.toolname` (e.g. `github.list_issues`).
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
    unsafe_host_execution: true,
    extra_env: Default::default(),
});
let mut mgr = InMemoryWorkerManager::new(Box::new(backend));
let slot = mgr.ensure(&session, &binding, "github")?;
```

## Tests

- Unit: ensure/teardown, synthetic call, TOML `upstream` parse
- Integration: mock Python NDJSON MCP server → handshake + `tools/call`
- Composite: mixed synthetic + upstream on one binding; session focus tears down old workers
- Protocol adversarial: alternate account/org/project/team selectors never reach the worker; injected credential canaries never return
