# locus-mcp — Claude Code, Cursor, and friends

`locus-mcp` is a **stdio MCP server** that exposes Locus control tools and **only** the provider tools for the **currently pinned** binding. It is the agent-facing half of the identity plane.

CLI (`locus pin`, `locus exec`) and MCP share the same store and seal under `~/.locus` (or `LOCUS_HOME`).

## Install

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo install --path crates/locus-cli
cargo install --path crates/locus-mcp
```

Or use release binaries from GitHub Releases (`locus` + `locus-mcp` for your target).

Confirm:

```bash
which locus locus-mcp
locus --help
```

## Human workflow (pin first)

Agents **cannot** pin a session. A human (or CI step) must:

```bash
locus init --with-samples   # first time
locus pin acme              # seal session to binding "acme"
locus whoami                # verify tenant + providers
```

Until a valid pin exists, `tools/list` returns **control tools only** (`locus_whoami`, `locus_status`, `locus_list_bindings`, `locus_request_pin`). There is **no** ambient fallthrough to personal accounts.

## Wire into Claude Code

From the project (or home) directory where you want MCP config:

```bash
locus pin acme
locus setup --client claude
```

This writes/merges `.mcp.json` so Claude Code launches `locus-mcp` over stdio. **Restart Claude Code** after setup so the tool catalog reloads.

Manual equivalent (illustrative):

```json
{
  "mcpServers": {
    "locus": {
      "command": "locus-mcp",
      "args": []
    }
  }
}
```

Ensure `locus-mcp` is on `PATH` for the GUI app (launchers sometimes inherit a minimal env — use absolute path if needed).

## Wire into Cursor

```bash
locus setup --client cursor
```

Cursor MCP config is typically user-level JSON; setup merges a `locus` server entry pointing at `locus-mcp`. Restart Cursor after changes.

## Wire into Codex / other stdio MCP clients

```bash
locus setup --client codex
```

Any client that can spawn a stdio MCP server can use:

| Field | Value |
|-------|--------|
| Transport | stdio |
| Command | `locus-mcp` |
| Args | (none) |
| Env | optional `LOCUS_HOME` for isolated stores |

## Protocol notes

- **JSON-RPC 2.0** over stdio — **Content-Length** framing (Claude Code / Cursor) and **NDJSON**  
- Supported methods: `initialize`, `ping`, `tools/list`, `tools/call`, `resources/list`, `resources/read`, `prompts/list`, `prompts/get`  
- **Never** log to stdout from the server — that breaks the protocol (errors go to stderr)  
- Protocol version advertised: `2024-11-05`  
- `initialize.instructions` carries crisp agent rules (whoami first, cannot pin, scope freeze, resources)

## Tools the agent sees

Every tool description is prefixed with **`[locus:<alias|unpinned>]`** so the model always sees which tenant the catalog belongs to. **`locus_whoami` is always first** in `tools/list`.

### Always (control plane)

| Tool | Behavior |
|------|----------|
| `locus_whoami` | Active pin: tenant, binding, providers, frozen scopes — **no secrets** |
| `locus_status` | Short pinned/unpinned + seal status |
| `locus_heartbeat` | Doctor-lite / runtime drift (seal, freeze, binding match) |
| `locus_enter_hint` | Shell command for the human to pin (`locus enter …`) |
| `locus_list_bindings` | Configured aliases/tenants |
| `locus_request_pin` | Returns instructions for the human; **does not pin** |

### When pinned

| Tool | Behavior |
|------|----------|
| `locus_providers` | Providers + frozen scopes for this pin |
| `supabase.*` / `github.*` / `vercel.*` | Adapter tools for providers on the binding |

Tool names are **not** multi-tenant namespaced under exclusive pin — the catalog itself is the isolation boundary. Wrong-tenant tools simply do not appear.

## Resources

| URI | Content |
|-----|---------|
| `locus://session` | Current pin whoami JSON (or `{ "pinned": false }`) — **no secrets** |
| `locus://doctor` | Doctor report (runtime drift, pin, workspace, findings) — **no secrets** |
| `locus://bindings` | Binding summaries (alias, tenant, providers) |

Use `resources/list` + `resources/read` with `{ "uri": "locus://session" }`.

## Prompts

| Name | Purpose |
|------|---------|
| `locus_context` | System prompt fragment: active tenant, frozen scopes, **you cannot pin — ask the human** |

`prompts/get` with `{ "name": "locus_context" }` returns a user-role message agents can inject as context.

## MCP auto-pin (cwd / workspace)

Agents still **cannot** call pin. The **server** may silently pin once at MCP start (and at most once per process) from the workspace when policy allows:

| Enable signal | Effect |
|---------------|--------|
| `.locus.toml` has `default_binding` | **Preferred default** — auto-pin that binding on MCP start (cwd must see the workspace file) |
| `.locus.toml` has `require_pin = true` | Enables auto-pin policy (still needs a resolvable default or autopin target) |
| `LOCUS_MCP_AUTO_PIN=1` | Explicit enable |
| `LOCUS_AUTO_PIN=cwd` or `clients.auto_pin = "cwd"` in `$LOCUS_HOME/config.toml` | Enable cwd-based auto-pin |
| `LOCUS_MCP_AUTO_PIN=0` / `false` / `off` | **Kill switch** — never auto-pin |

Rules:

1. Only when **unpinned**; never rewrites a human pin mid-session.  
2. Only when workspace has `require_pin` or non-empty `default_binding`.  
3. Uses `pin_auto` — **never `--force`**; workspace `allowed_bindings` always wins.  
4. Audits **`session.auto_pin`** (plus normal `session.pin`).  
5. Fail soft: if pin fails, stay unpinned and expose control tools only.

Example workspace:

```toml
# .locus.toml
version = 1
default_binding = "acme"
allowed_bindings = ["acme"]
require_pin = true
```

```bash
# optional explicit enable for CI / non-workspace shells
export LOCUS_MCP_AUTO_PIN=1
# kill switch when you want a bare control catalog
export LOCUS_MCP_AUTO_PIN=0
```

## Scope freeze and policy

1. **Policy** (`binding.policy`) may deny or require approval (`confirm=true`) before the adapter runs.  
2. **Scope freeze** rejects args that conflict with frozen selectors (e.g. another Supabase `project_ref`).  
3. Results are synthetic/identity-oriented in phase 1 — safe to explore without mutating cloud resources.

Agent best practice: call `locus_whoami` before any infrastructure work. If unpinned or wrong tenant, ask the human to `locus pin <alias>`.

## Switching clients

```bash
locus pin personal    # human action
# restart or rely on next tools/list — catalog follows the new pin
locus leave           # unbind; agent loses provider tools
```

Workspace defaults (`.locus.toml`) affect CLI pin UX. MCP reads the **active sealed session** under `LOCUS_HOME`; with auto-pin enabled (see above) it may also pin the workspace `default_binding` once when the server’s cwd contains that `.locus.toml`.

## Isolation relative to other MCPs

| Anti-pattern | Locus approach |
|--------------|----------------|
| N Supabase MCP servers (personal + each client) in one agent | One `locus-mcp`; pin selects which project_ref exists |
| Global `gh auth` shared by all chats | Pin + private worker dirs for `locus exec`; MCP tools report frozen GH scope |
| Model “please use the other project” | Freeze denies alternate selectors |

Do **not** also register unrestricted personal Supabase/GitHub MCP servers alongside Locus if you want hard isolation — that reintroduces ambient tools outside the pin.

## Troubleshooting

| Symptom | Check |
|---------|--------|
| Only `locus_*` tools | No valid pin — run `locus pin` / `locus whoami`; seal may be expired or tampered |
| `locus-mcp` not found | PATH for GUI vs terminal; reinstall; absolute path in MCP config |
| Wrong tenant tools | `locus whoami`; re-pin; inspect binding providers |
| Scope freeze errors | Model tried to override frozen `project_ref` / team / org — expected |
| Setup didn’t stick | Re-run `locus setup --client …`; confirm config file path for your client version |

```bash
locus doctor
locus status
LOCUS_HOME=/tmp/locus-debug locus pin personal
# point a test MCP client at locus-mcp with LOCUS_HOME set the same way
```

## Security properties (MCP-specific)

- Agents cannot re-pin or elevate via tools.  
- MCP responses never include resolved secret material.  
- Invalid session seal → errors; fail closed.  
- See [SECURITY.md](../SECURITY.md) and [DESIGN.md §9](../DESIGN.md).

## Related

- [docs/adapters.md](./adapters.md) — add provider tools  
- [README.md](../README.md) — CLI quick start  
- [CONTRIBUTING.md](../CONTRIBUTING.md) — build and test  
