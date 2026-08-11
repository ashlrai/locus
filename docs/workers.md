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

## Worker sandbox (best-effort / platform backends)

Blast radius for a malicious upstream is still **one binding’s credentials**. Sandbox is additive isolation, **not** a multi-tenant VM or full seccomp profile. When enabled, Locus selects a platform backend and records it in `LOCUS_WORKER_SANDBOX_BACKEND`. **Never invent false security:** read the backend tag — `path` is not equivalent to Seatbelt or bubblewrap.

Enable globally:

```bash
export LOCUS_WORKER_SANDBOX=1
```

Or per-provider in binding TOML:

```toml
upstream = { recipe = "github-mcp", resolve_secrets = true, sandbox = true }
```

### Opt-in network isolation (harder sandbox)

Default remains **network allowed** so MCP workers can call provider APIs. For offline fixtures, local tools, or deliberate hard isolation:

```bash
export LOCUS_WORKER_SANDBOX=1
export LOCUS_WORKER_SANDBOX_NO_NETWORK=1
```

Or per-provider:

```toml
upstream = { recipe = "filesystem-mcp", sandbox = true, sandbox_no_network = true }
```

| Backend | Effect when no-network is set |
|---------|-------------------------------|
| `bwrap` | Adds `--unshare-net` (real net namespace) |
| `sandbox-exec` | Omits `(system-network)` / outbound TCP·UDP allows (macOS best-effort under imported `system.sb`) |
| `path` | **Fail closed** — cannot enforce network isolation; install bubblewrap or disable the flag |

No-network has no effect unless sandbox is also on.

### Backend matrix

| Platform | Tag | Strength | When |
|----------|-----|----------|------|
| macOS | `sandbox-exec` | Seatbelt deny-by-default | `/usr/bin/sandbox-exec` required; missing → fail closed |
| Linux | `bwrap` | bubblewrap mount + pid namespace (best-effort) | Prefer when `bwrap` is on a fixed path (`/usr/bin/bwrap`, `/bin/bwrap`, `/usr/local/bin/bwrap`) |
| Linux | `path` | Restricted PATH + absolute executable only | Fallback when `bwrap` is missing — **not** kernel isolation |
| other | — | — | Fail closed |

### Spawn steps (all backends)

| Step | Behavior |
|------|----------|
| Backend | Platform selection above; tag written to `LOCUS_WORKER_SANDBOX_BACKEND` |
| Files (Seatbelt / bwrap) | Work tree + session worker home RW; system roots RO; no bind/grant of full `LOCUS_HOME` (bindings, `daemon.key`, sessions, approvals, audit stay out) |
| Files (`path`) | No mount namespace — only PATH restriction + canonical absolute executable |
| Authority | Work trees that overlap or contain `LOCUS_HOME` are refused before spawn |
| Secrets | Rebuilds env from the isolation allowlist and uses a private temp root under the worker home |
| Network | **Allowed by default** for MCP stdio → provider APIs (Seatbelt: outbound TCP/UDP; bwrap: shared netns — no `--unshare-net`; path: host network). Opt-in deny: `LOCUS_WORKER_SANDBOX_NO_NETWORK=1` or `upstream.sandbox_no_network = true` → bwrap `--unshare-net` / Seatbelt omit outbound allows (macOS best-effort). The Linux `path` backend **fails closed** if no-network is requested (cannot enforce). |
| Provenance | Resolves the requested executable to a canonical absolute path before PATH is restricted; unavailable commands fail before spawn |
| Marker | Sets `LOCUS_WORKER_SANDBOXED=1` and `LOCUS_WORKER_SANDBOX_BACKEND=<tag>` only after backend resolution succeeds; when no-network applies, also sets `LOCUS_WORKER_SANDBOX_NO_NETWORK=1` on the child |

### Linux bubblewrap profile (when `bwrap` is used)

Minimal session-sized profile (not a VM):

- `--ro-bind-try` for `/usr`, `/bin`, `/lib`, `/lib64`, `/sbin`, `/usr/local`, `/etc` (TLS/DNS)
- `--bind` work directory + session worker home only (not `~/.locus/bindings` or host home)
- `--tmpfs /tmp`, `--proc`, `--dev`, `--unshare-pid`, `--die-with-parent`
- Network **shared by default** so upstream MCP can reach provider APIs
- When `LOCUS_WORKER_SANDBOX_NO_NETWORK=1` / `sandbox_no_network = true`: adds `--unshare-net`
- Extra runtime roots (e.g. node package trees outside `/usr`) bound RO as needed

Bubblewrap still depends on host user namespaces and may fail at spawn on locked-down kernels — that is a hard error, not a silent downgrade. If `bwrap` is simply **not installed**, Locus uses the `path` backend instead and tags it clearly.

Composite uses the same flag path: `mcp_config_from_upstream` sets `McpStdioConfig.sandbox` from the spec **or** env.

Worker discovery and startup are fail closed: `tools/list` never starts an
upstream child or resolves its credentials. An authorized `tools/call` starts
only the addressed provider, whose environment contains only that provider's
scope and credential keys. Batch/session startup is transactional; if a later
provider fails, every child created earlier in that attempt is torn down.

This is **not** a VM boundary. The explicitly allowed work tree may itself contain sensitive files, so bindings should use a narrowly scoped working directory.
See SECURITY.md / DESIGN.md for residual risk: a started worker already holds
the addressed provider's scoped credential.

## Binding TOML — per-provider upstream

```toml
[[binding.providers]]
provider = "github"
account = "acme"
credential_ref = "phm:GH_TOKEN_ACME"
scope = { orgs = ["acme-corp"] }
# Sandbox-compatible npx recipe — expands to command/args + hardened defaults:
upstream = { recipe = "github-mcp", resolve_secrets = true, sandbox = true }
# Official Docker recipe requires explicit sandbox = false acknowledgement.
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
| `github-official` | Official Docker image + `GITHUB_PERSONAL_ACCESS_TOKEN`; unavailable by default because Docker daemon authority cannot be sandboxed | explicit `sandbox = false` |
| `github-mcp` | Legacy community `@modelcontextprotocol/server-github` via `npx` (deprecated package) | `resolve_secrets`, `sandbox` |
| `supabase-mcp` | `@supabase/mcp-server-supabase` stdio (`--read-only`) | `resolve_secrets`, `sandbox` |
| `vercel-mcp` | Official remote `https://mcp.vercel.com` via `mcp-remote`; unavailable by default because first-time OAuth needs a loopback listener and Locus cannot attest cached auth separately | explicit `sandbox = false` |
| `filesystem-mcp` | Safe filesystem demo (override root path via `args`) | off |
| `everything-mcp` | MCP test/echo server for wiring checks | off |

```bash
locus upstream list
locus upstream suggest github
locus upstream suggest supabase
locus upstream suggest vercel
```

Recipe table source: [`adapters/recipes.toml`](../adapters/recipes.toml). Explicit `command` / `args` replace the recipe's command or arguments, but do not disable its sandbox policy. Compatible recipes adopt `default_sandbox`, including command-only and args-only overrides; explicit `sandbox = false` remains the opt-out. Recipes whose machine-readable `readiness` is `explicit_unsandboxed_required` are unavailable when `sandbox` is omitted or true, and run only after an explicit `sandbox = false` acknowledgement. `LOCUS_WORKER_SANDBOX=1` makes those recipes fail closed instead of producing a false sandbox claim.

The macOS Seatbelt profile permits outbound TCP/UDP provider connections, but denies application-created inbound listeners and non-system Unix-domain sockets. It imports Apple's `system.sb` and `system-network` baseline and allows `system-socket`, so OS-defined system-service IPC such as logging is intentionally outside the blanket denial claim. Native tests prove denial for an arbitrary external Unix socket, `/var/run/docker.sock` when present, Docker Desktop-style sockets, and TCP OAuth listeners. Locus never grants a Docker socket or blanket inbound networking. Docker therefore remains a host-level, high-authority execution path outside the filesystem boundary. The Vercel bridge remains unsandboxed until Locus can separate and attest OAuth bootstrap from a cached-auth steady state; host-native remote MCP is preferred meanwhile.

**Remote URLs (host MCP, not Locus workers):** Supabase `https://mcp.supabase.com/mcp` (optional `?project_ref=…`); Vercel `https://mcp.vercel.com` (OAuth). Locus upstream workers are **stdio** only today. Use host-native remote MCP when the client supports it; the `vercel-mcp` bridge is an explicit unsandboxed fallback.

## Composite + top-adapter recipes

`CompositeWorkerManager` (used by `locus-mcp`) expands binding `upstream` via
`mcp_config_from_upstream` before spawn:

| Binding | Expanded defaults (pure recipe) |
|---------|----------------------------------|
| `upstream = { recipe = "github-mcp" }` | `npx` + `@modelcontextprotocol/server-github`, `resolve_secrets=true`, `sandbox=true` |
| `upstream = { recipe = "supabase-mcp" }` | `npx` + `@supabase/mcp-server-supabase@latest --read-only`, same hardened defaults |
| `upstream = { recipe = "vercel-mcp" }` | **unavailable** until `sandbox = false` (OAuth loopback); secrets default off |
| `upstream = { recipe = "github-official" }` | **unavailable** until `sandbox = false` (Docker daemon) |

Sibling providers without `upstream` stay **synthetic** (freeze/scope tools only).
`ensure_provider` starts **only** the addressed provider — an allow decision for
github never resolves supabase credentials. Catalog remains **exclusive** to the
pinned binding: no ambient personal tools, no cross-binding fallthrough.

When `resolve_secrets = false` (non-pure override, or recipes that default off),
the child env is rebuilt from the isolation allowlist with `env_clear` — ambient
`GH_TOKEN` / `GITHUB_PERSONAL_ACCESS_TOKEN` / `SUPABASE_ACCESS_TOKEN` from the
parent process do **not** enter the worker. Pure recipes that default
`resolve_secrets` on still inject only that provider’s resolved CredentialRef
keys after scrub; they never forward parent env wholesale.

When `locus-mcp` is pinned:

1. `tools/list` returns synthetic schemas plus schemas cached from workers that an earlier authorized call already started.
2. `tools/call` completes session, scope, and approval policy checks before `ensure_provider` starts only the addressed upstream child.
3. Upstream tools are namespaced as `provider.toolname` (e.g. `github.list_issues`).
4. Synthetic tools (e.g. `github.scope`) stay available; name collisions prefer synthetic.

Worker children start from a positive runtime environment allowlist. Locus then
adds frozen binding metadata and only the provider credential keys resolved for
that provider. `McpStdioConfig::extra_env` remains for source compatibility but
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
    sandbox_no_network: false, // or true / LOCUS_WORKER_SANDBOX_NO_NETWORK=1
});
let mut mgr = InMemoryWorkerManager::new(Box::new(backend));
let slot = mgr.ensure(&session, &binding, "github")?;
```

## Worker pool reuse

`CompositeWorkerManager` is process-wide inside `locus-mcp`. For a given pin:

1. An authorized `tools/call` calls `ensure_provider`; explicit batch callers may use transactional `ensure_session` / `ensure_binding`.
2. Existing slots in `Ready` / `Running` / `Pending` are **reused** — upstream MCP children are **not** respawned per list/call.
3. Pin switch (`focus_session`) tears down slots for other `session_id`s.
4. Process exit / `Drop` tears down all remaining children.

### Optional idle timeout

```bash
export LOCUS_WORKER_IDLE_SECS=300   # tear down workers idle for 5 minutes
export LOCUS_WORKER_SANDBOX=1       # platform backend (sandbox-exec / bwrap / path)
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
