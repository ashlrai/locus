# locus-mcp — Claude Code, Cursor, and friends

`locus-mcp` is a **stdio MCP server** that exposes Locus control tools and **only** the provider tools for the **currently pinned** binding. It is the agent-facing half of the identity plane.

CLI and MCP share the same store and seal under `~/.locus` (or `LOCUS_HOME`).
`locus exec --no-resolve`, `locus run --no-resolve`, and
`locus ci run --no-resolve` are manual,
identity-only diagnostics: they cannot resolve provider credentials or spawn
credential-resolving provider workers, and Hub or agent-originated sessions
cannot invoke them. All three use one recipe-expanded preflight and fail before
child, worker, session, or credential effects when any declared upstream
expands to `resolve_secrets = true` (including recipe defaults).
Credential-free upstream declarations are permitted and remain usable.

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
- `initialize.instructions` carries crisp agent rules (whoami / `locus_safe_next` first, cannot pin, scope freeze, live pin state after auto-pin, resources)

## HTTP transport (CI / remote agents)

Stdio remains the default for Claude Code and Cursor. For CI runners and agents that prefer HTTP, enable the **streamable-HTTP-lite** server (JSON-RPC over HTTP with streamable Accept negotiation + multi-message SSE for large results):

```bash
export LOCUS_MCP_HTTP_TOKEN="$(openssl rand -hex 16)"   # required shared secret
locus-mcp --http                    # binds 127.0.0.1:8742
# or:
locus-mcp --http 127.0.0.1:9000
# or:
LOCUS_MCP_HTTP=1 LOCUS_MCP_HTTP_ADDR=127.0.0.1:8742 locus-mcp
```

| Endpoint | Auth | Behavior |
|----------|------|----------|
| `GET /health` (`/healthz`, `/`) | none | Liveness JSON: `{ ok, service, version, transport, endpoints }` |
| `GET /mcp` (also `/jsonrpc`) | **required** | Capabilities + pin summary + **tool names only** (values-free); advertises `Mcp-Session-Id` support |
| `GET /mcp/sse` | **required** | Long-lived SSE hub heartbeat: `locus.session_tick` (`session_ok`, doctor verdict, safe_next). Query: `?once=1`, `?interval=5s` |
| `POST /mcp` (also `/jsonrpc`) | **required** | One JSON-RPC 2.0 request → JSON, single SSE event, or **multi-message SSE** for large bodies; mints/binds `Mcp-Session-Id` |
| `DELETE /mcp` | **required** | Terminate the session named by `Mcp-Session-Id` (**204** / **404**) |
| `OPTIONS /mcp` | none | Minimal CORS preflight for local tooling |

Auth headers (any one):

- `Authorization: Bearer <LOCUS_MCP_HTTP_TOKEN>`
- `X-Locus-Token: <token>`
- `X-Locus-Mcp-Token: <token>`

### `Mcp-Session-Id` (in-memory)

Streamable clients get an opaque process-local session id (MCP streamable HTTP). Storage is **in-process only** (no Redis / disk).

| Step | Behavior |
|------|----------|
| `POST /mcp` **without** `Mcp-Session-Id` | Mint a new opaque id (initialize / first POST path). Response includes `Mcp-Session-Id: <id>`. |
| `POST /mcp` **with** a known id | Bind to that session, refresh idle TTL, echo the same header. |
| Unknown / expired id | **404** `{ "error": "unknown_session", ... }` — fail closed. |
| Empty id | **400** `{ "error": "invalid_session", ... }`. |
| `DELETE /mcp` + id | Drop the session (**204**); further use of that id → **404**. |
| Capacity | Hard cap (default 256). Further mints → **503** `session_capacity`. |
| Idle TTL | Default **30 minutes** from last successful bind. |

`GET /mcp` does **not** require a session (capabilities probe). If a client sends `Mcp-Session-Id` on GET, unknown ids still fail closed with **404**. Capabilities JSON lists `streamable.session` (`header`, `ttl_seconds`, `max_sessions`, mint rule).

This is **not** multi-process affinity — restarting `locus-mcp` drops the map. Multi-message SSE (progress/chunks + final) is available on POST when Accept allows event-stream; cross-process session resume remains open.

### Streamable Accept / Content-Type

Compatible with MCP streamable HTTP. Partial multi-message SSE (not a full remote multiplexor rewrite):

| Header | Server behavior |
|--------|-----------------|
| `Content-Type: application/json` | Required for POST when present; missing Content-Type still accepted (legacy CI). Non-JSON → **415**. |
| `Accept: application/json` (and/or `*/*`) | Response `Content-Type: application/json` (default / preferred for small bodies). |
| `Accept: text/event-stream` only | Response `Content-Type: text/event-stream`. Small bodies → **one** `event: message`. Large bodies / large `tools/call` → **multi-message** stream (see below). |
| `Accept` lists both | Prefer JSON for small responses; **upgrade to multi-message SSE** when the JSON-RPC body exceeds `LOCUS_MCP_SSE_MULTI_BYTES` (default 4096). |
| `Accept` that allows neither | **406** not acceptable. |
| `Mcp-Session-Id` | See table above; echoed on successful POST (and GET when provided + valid). |

#### Multi-message SSE (POST `/mcp`)

When SSE is selected and the response is large (or `tools/call` crosses the soft threshold):

1. `event: message` + `notifications/message` with `data.kind = "locus.sse.progress"` (start)
2. Optional progressive chunks: `data.kind = "locus.sse.chunk"` with `{ index, total, text }` for large tool text
3. Final `event: message` carrying the **complete** JSON-RPC response (`id` + full `result`/`error`)

Header `X-Locus-Streamable: sse-multi` (or `sse-single` for one-event replies). Intermediate events are JSON-RPC notifications (no `id`); only the final event is the authoritative response. Env: `LOCUS_MCP_SSE_MULTI_BYTES`, `LOCUS_MCP_SSE_CHUNK_BYTES`. Successful responses also echo `Mcp-Session-Id` when a session was minted/bound.

#### Session SSE (GET `/mcp/sse`)

Hub-friendly continuous identity ticks without CLI `locus watch`:

```bash
curl -N -H "Authorization: Bearer $LOCUS_MCP_HTTP_TOKEN"   -H "Accept: text/event-stream"   "http://127.0.0.1:8742/mcp/sse?interval=5s"
# one-shot smoke:
curl -N -H "Authorization: Bearer $LOCUS_MCP_HTTP_TOKEN"   -H "Accept: text/event-stream"   "http://127.0.0.1:8742/mcp/sse?once=1"
```

Each tick is a JSON-RPC `notifications/message` with values-free `data`:

```json
{
  "kind": "locus.session_tick",
  "session_ok": true,
  "whoami": "acme",
  "doctor_verdict": "SAFE",
  "safe_next": "ready",
  "pinned": true,
  "frozen": false
}
```

`X-Locus-Streamable: sse-session`. Interval default 5s (`LOCUS_MCP_SSE_INTERVAL` or `?interval=`). Never includes secrets.

Cross-process `Mcp-Session-Id` resume and multi-tenant remote multiplexor remain open.

**Hard rules:**

1. **Loopback only by default** — non-`127.0.0.1` / non-loopback binds refuse unless `LOCUS_MCP_HTTP_ALLOW_REMOTE=1`.
2. **Token required** — HTTP mode will not start without a non-empty `LOCUS_MCP_HTTP_TOKEN`.
3. Missing/wrong token → **401**; never soft-allow. Applies to `GET /mcp`, `GET /mcp/sse`, `POST /mcp`, and `DELETE /mcp`.
4. Unknown / expired `Mcp-Session-Id` → **404**; never soft-allow.
5. Same pin/policy/seal gate as stdio — HTTP is only a transport.
6. `GET /mcp` / session ticks never return secret values or credential refs — tool **names** and pin alias/tenant only.

Example (CI):

```bash
curl -sS http://127.0.0.1:8742/health
curl -sS -H "Authorization: Bearer $LOCUS_MCP_HTTP_TOKEN" \
  http://127.0.0.1:8742/mcp
# Initialize / first POST mints Mcp-Session-Id (inspect response headers):
curl -sS -D - -H "Authorization: Bearer $LOCUS_MCP_HTTP_TOKEN" \
  -H "Content-Type: application/json" \
  -H "Accept: application/json, text/event-stream" \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"ci","version":"0"}}}' \
  http://127.0.0.1:8742/mcp
# Subsequent RPC with the minted id:
curl -sS -H "Authorization: Bearer $LOCUS_MCP_HTTP_TOKEN" \
  -H "Content-Type: application/json" \
  -H "Mcp-Session-Id: $SESSION_ID" \
  -d '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}' \
  http://127.0.0.1:8742/mcp
# SSE-only Accept (may multi-message for large tools/list):
curl -N -H "Authorization: Bearer $LOCUS_MCP_HTTP_TOKEN" \
  -H "Content-Type: application/json" \
  -H "Accept: text/event-stream" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}' \
  http://127.0.0.1:8742/mcp
```

### Remote deploy (reverse proxy)

HTTP mode is safe for **remote** agents only when you treat the process like any other secret-scoped service:

1. **Pin before serving** — identity is resolved at the gate. On the host that runs `locus-mcp`:
   ```bash
   export LOCUS_HOME=/var/lib/locus          # dedicated store (not a shared laptop home)
   locus init --with-samples                # once
   locus pin <alias>                        # or: locus ci mint / enter workflow
   locus whoami                             # confirm tenant
   ```
   Agents still **cannot** pin. Wrong pin ⇒ wrong catalog. Unpinned ⇒ control tools only.

2. **Token** — set a strong `LOCUS_MCP_HTTP_TOKEN` (env or secret manager). Never commit it. Rotate on offboarding.

3. **Bind loopback + reverse proxy** (recommended):
   ```bash
   # process listens only on loopback
   LOCUS_MCP_HTTP_TOKEN=… LOCUS_HOME=/var/lib/locus \
     locus-mcp --http 127.0.0.1:8742
   ```
   Terminate TLS and auth at the proxy (nginx, Caddy, Envoy, cloud LB). Forward:
   - `Authorization` (or `X-Locus-Token`)
   - `Content-Type`, `Accept`
   - `Mcp-Session-Id` (and expose it on responses for browser clients)
   - path `/mcp` and `/health` (health may stay internal)

   Example nginx sketch:
   ```nginx
   location /mcp {
     proxy_pass http://127.0.0.1:8742;
     proxy_set_header Authorization $http_authorization;
     proxy_set_header Content-Type $http_content_type;
     proxy_set_header Accept $http_accept;
     proxy_set_header Mcp-Session-Id $http_mcp_session_id;
     proxy_pass_header Mcp-Session-Id;
     proxy_http_version 1.1;
   }
   location /health {
     proxy_pass http://127.0.0.1:8742/health;
     # optional: restrict to VPC / private probes only
   }
   ```

4. **Non-loopback bind** — only if you understand the exposure:
   ```bash
   LOCUS_MCP_HTTP_ALLOW_REMOTE=1 locus-mcp --http 0.0.0.0:8742
   ```
   Still require the token; prefer network policy (security group / private subnet) in front. TLS at the edge is mandatory for anything beyond localhost.

5. **LOCUS_HOME isolation** — each remote multiplexor process should use its own `LOCUS_HOME` (or a carefully shared store with one intended pin). Do not point a shared CI runner at a developer's `~/.locus`.

6. **Pin requirements** — same as stdio: workspace `.locus.toml` `allowed_bindings` / `require_pin` apply; provider tools appear only with a healthy sealed pin. Destructive tools still hit policy / external authority.

7. **Do not log tokens or resolved secrets** — MCP never returns secret values; keep proxy access logs free of `Authorization` bodies when possible.

### Capability tickets

On each provider `tools/call` path, locus-mcp mints a short-lived **capability ticket**:

```text
material = session_id|binding_id|tool|exp
ticket_id = cap_ + hex(HMAC-SHA256(daemon_key, material))
TTL       = 30s (default)
```

- `ticket_id` is written to the audit event `mcp.tools_call` — it is **not** a secret and is safe in logs.
- Verify with the store: `Store::verify_capability_ticket` / `verify_capability_ticket_parts` (same daemon key under `LOCUS_HOME`).
- Tickets do not replace the seal or policy; they correlate a call for forensics / future worker side-channels.

See also [docs/workers.md](./workers.md) for worker pool reuse and idle teardown.

## Tools the agent sees

Every tool description is prefixed with **`[locus:<alias|unpinned>]`** so the model always sees which tenant the catalog belongs to. **`locus_whoami` is always first** in `tools/list`.

### Always (control plane)

| Tool | Behavior |
|------|----------|
| `locus_whoami` | Active pin: tenant, binding, providers, frozen scopes — **no secrets** |
| `locus_safe_next` | **Single best next action** (enter / re-pin / approval blocked / doctor fix / ready) — call when stuck |
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

Agents still **cannot** call pin. The **server** may silently pin once at MCP start (and at most once per process) from the workspace when policy allows.

`locus agent setup --apply` writes `.locus/AGENT.md` with this table and sets `LOCUS_AUTO_PIN=cwd` on MCP client env.

### Enable signals

| Signal | Effect |
|--------|--------|
| `.locus.toml` has `default_binding` | **Preferred default** — auto-pin that binding on MCP start (cwd must see the workspace file) |
| `.locus.toml` has `require_pin = true` | Enables auto-pin policy (still needs a resolvable default or autopin target) |
| `LOCUS_MCP_AUTO_PIN=1` / `true` / `on` | Explicit enable |
| `LOCUS_AUTO_PIN=cwd` or `clients.auto_pin = "cwd"` in `$LOCUS_HOME/config.toml` | Enable cwd-based auto-pin |

### Kill switches

| Signal | Effect |
|--------|--------|
| `LOCUS_MCP_AUTO_PIN=0` | **Kill switch** — never auto-pin (also `false` / `off` / `no`) |
| Omit workspace `default_binding` / `require_pin` and leave enable signals unset | Auto-pin policy stays off |

Kill switch **wins** over workspace `default_binding` and `LOCUS_AUTO_PIN=cwd`.

Rules:

1. Only when **unpinned**; never rewrites a human pin mid-session.  
2. Only when workspace has `require_pin` or non-empty `default_binding`.  
3. Uses `pin_auto` — **never `--force`**; workspace `allowed_bindings` always wins.  
4. Audits **`session.auto_pin`** (plus normal `session.pin`).  
5. Fail soft: if pin fails, stay unpinned and expose control tools only.  
6. After auto-pin, **tools / resources / prompts** all re-read live pin state (`listChanged` advertised; re-read `locus://session` / `locus_context`).

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

1. **Policy** (`binding.policy`) may deny or require external authority before the adapter runs. Local `confirm`, approval ids, CLI labels, HTTP tokens, and dashboard actions are advisory only.
2. **Scope freeze** rejects args that conflict with frozen selectors (e.g. another Supabase `project_ref`).  
3. Results are synthetic/identity-oriented in phase 1 — safe to explore without mutating cloud resources.

Agent best practice: call `locus_whoami` or `locus_safe_next` before any infrastructure work. If unpinned or wrong tenant, ask the human to `locus pin <alias>`. When blocked (approval, freeze, doctor), prefer `locus_safe_next` for the single next action.

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
| Global `gh auth` shared by all chats | Pin + private worker dirs; manual exec is credential-free and MCP tools report frozen GH scope |
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
