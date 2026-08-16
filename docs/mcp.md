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
locus init --with-samples   # first time — also mints LOCUS_CONTROL_CAPABILITY
                            # if missing (persisted 0600 under ~/.locus/)
locus pin acme              # seal session to binding "acme"
locus whoami                # verify tenant + providers
```

Control commands (init/quickstart/enter/pin/leave) require the operator
control capability `LOCUS_CONTROL_CAPABILITY` (64 lowercase hex chars) in the
shell env. `locus quickstart` / `locus init` mint and persist one when nothing
exists; export it in new shells with `eval "$(locus hook zsh)"` (reads
`$LOCUS_HOME/control_capability`, never echoes the value) or manage it
yourself: `export LOCUS_CONTROL_CAPABILITY="$(openssl rand -hex 32)"`.
`locus doctor` flags a missing, invalid, or mismatched capability with the
exact fix. locus-mcp deliberately runs **without** it — agents never hold
control authority. Persisting the capability is a deliberate onboarding
default: any same-user process can then run control commands. For the strict
posture use `locus init --no-persist-capability` or `locus capability
unpersist` (keep the printed export line in your shell profile); `locus
capability status` shows the current posture without printing the value. See
[SECURITY.md § Control-plane authority boundary](../SECURITY.md#control-plane-authority-boundary).

Until a valid pin exists, `tools/list` returns **control tools only** (`locus_whoami`, `locus_safe_next`, `locus_status`, `locus_heartbeat`, `locus_enter_hint`, `locus_list_bindings`, `locus_request_pin`, `locus_verify_claim`, `locus_verify_session`). There is **no** ambient fallthrough to personal accounts.

## Wire into Claude Code

From the project (or home) directory where you want MCP config:

```bash
locus pin acme
locus setup --client claude
```

This writes/merges `.mcp.json` so Claude Code launches `locus-mcp` over stdio. **Restart Claude Code** after setup so the tool catalog reloads.

To register once for **all** projects (Claude Code *user scope*), let the
claude CLI do the write — user-scope servers live in `~/.claude.json`, a
mixed-state file owned by Claude Code that Locus never hand-edits:

```bash
locus agent setup --apply --client claude --claude-scope user
```

This shells out to `claude mcp add-json locus '<entry>' --scope user`
(healing a stale entry via `claude mcp remove` first) and verifies with
`claude mcp get locus`. It requires the `claude` CLI on PATH and fails closed
with instructions when it is missing. Default `--claude-scope project` keeps
the project-local `.mcp.json` behavior. Scope precedence in Claude Code is
local > project > user, matched by server name.

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

## Wire into Grok Build

Grok Build's documented MCP config is `~/.grok/config.toml` — Codex-style
`[mcp_servers.<name>]` TOML tables. Locus writes it with the same fail-closed
`toml_edit` merge used for Codex (unparseable file ⇒ abort untouched; only the
`locus` entry is upserted):

```bash
locus setup --client grok
locus agent setup --apply --client grok   # grok is also included in --client all
```

The entry carries `LOCUS_AUTO_PIN=cwd` + `LOCUS_CLIENT=grok` (never
`LOCUS_NOTIFY`). Restart Grok Build (or `/mcps` in its TUI) after changes;
`grok mcp list` / `grok inspect` show the loaded server. Note Grok Build also
compat-loads MCP configs from `~/.claude.json`, project `.mcp.json`, and
`.cursor/mcp.json`, so Locus's Claude/Cursor writes are typically already
visible to it.

`locus agent doctor` / `locus agent report --json` verify the registration
(`mcp_registered.grok`) by probing `~/.grok/config.toml`. To probe a
nonstandard location (e.g. a compat mcp.json), override it:

```bash
export LOCUS_GROK_MCP_CONFIG=/path/to/config   # JSON mcpServers or TOML [mcp_servers]
```

## Any other stdio MCP client (generic)

For clients without a known on-disk config path, emit the canonical stdio
server entry (JSON **and** TOML shapes) and paste it into the client's MCP
settings — nothing is written:

```bash
locus setup --client generic
locus agent setup --dry-run --client generic
```

Point the client at `locus-mcp` (absolute path for GUI launchers with a
minimal PATH). `LOCUS_GROK_MCP_CONFIG` doubles as the probe override for such
clients so `mcp_registered.grok` can verify them; without a real config path
the probe never guesses.

## Protocol notes

- **JSON-RPC 2.0** over stdio — **Content-Length** framing (Claude Code / Cursor) and **NDJSON**  
- Supported methods: `initialize`, `ping`, `tools/list`, `tools/call`, `resources/list`, `resources/read`, `prompts/list`, `prompts/get`  
- **Never** log to stdout from the server — that breaks the protocol (errors go to stderr)  
- Protocol version advertised: `2024-11-05`  
- `initialize.instructions` carries crisp agent rules (whoami / `locus_safe_next` first, cannot pin, scope freeze, live operator-controlled pin state, resources)

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
| `POST /mcp` (also `/jsonrpc`) | **required** | One JSON-RPC 2.0 request → JSON, single SSE event, or **multi-message SSE** for large bodies; binds `Mcp-Session-Id` (mint on `initialize` only) |
| `DELETE /mcp` | **required** | Terminate the session named by `Mcp-Session-Id` (**204** / **404**) |
| `OPTIONS /mcp` | none | Minimal CORS preflight for local tooling |

Auth headers (any one):

- `Authorization: Bearer <LOCUS_MCP_HTTP_TOKEN>` (the `Bearer` scheme is
  required — a raw schemeless token in `Authorization` is rejected)
- `X-Locus-Token: <token>`
- `X-Locus-Mcp-Token: <token>`

**Pre-auth request limits** (fail closed before any body allocation or the token check): declared `Content-Length` above **8 MB** → **413**; request line + headers above **32 KB** total or more than **128** header fields → **431**. An unparseable JSON-RPC body returns **400** with JSON-RPC error `-32700` (parse error) — never a dropped connection, and **no** `Mcp-Session-Id` is minted.

### `Mcp-Session-Id` (memory + disk resume)

Streamable clients get an opaque session id (MCP streamable HTTP). Locus keeps an **in-memory cache** and a **file-backed map** so restarts and multiple `locus-mcp` workers on the same `LOCUS_HOME` can resume the same id.

| Step | Behavior |
|------|----------|
| `POST /mcp` `initialize` **without** `Mcp-Session-Id` | Mint a new opaque id (**`initialize` only**). Response includes `Mcp-Session-Id: <id>`. Persists under the session dir. |
| Non-`initialize` POST **without** `Mcp-Session-Id` | Served **statelessly** — no mint, so garbage POSTs cannot exhaust session capacity. Provider `tools/call` is still pin-swap protected via a shared **process-level anchor** (a fresh sessionless `initialize` re-anchors it). |
| `POST /mcp` **with** a known id | Bind to that session (memory hit, or **load-on-miss from disk**), refresh idle TTL, echo the same header. |
| Unknown / expired / **corrupt** id | **404** `{ "error": "unknown_session", ... }` — fail closed (corrupt files are removed, never soft-allowed). |
| Empty id | **400** `{ "error": "invalid_session", ... }`. |
| `DELETE /mcp` + id | Drop memory **and** disk (**204**); further use of that id → **404**. |
| Capacity | Hard cap (default 256 across memory + live disk files). Further mints → **503** `session_capacity`. |
| Idle TTL | Default **30 minutes** from last successful bind (wall clock; pruned on touch/mint). |

**Disk layout** (never secrets):

| Path | Contents |
|------|----------|
| `$LOCUS_HOME/http-sessions/<id>.json` | Default map (one file per id) |
| `LOCUS_MCP_SESSION_DIR/<id>.json` | Optional override of the session directory |

Each JSON file stores only: schema version, session `id`, `created_at_unix`, `last_seen_unix`, and optional **pin summary** (`binding_alias` / `tenant` / `mode` / `seal_ok`). Atomic write (temp + rename, mode `0600`). No tokens, credential refs, or resolved secrets.

`GET /mcp` does **not** require a session (capabilities probe). If a client sends `Mcp-Session-Id` on GET, unknown ids still fail closed with **404**. Capabilities JSON lists `streamable.session` (`header`, `ttl_seconds`, `max_sessions`, `storage`, mint rule).

Multi-message SSE (progress/chunks + final) is available on POST when Accept allows event-stream. Multi-tenant remote multiplexor remains open.

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

`X-Locus-Streamable: sse-session`. Interval default 5s (`LOCUS_MCP_SSE_INTERVAL` or `?interval=`). Never includes secrets. Ticks run the same external probes as `locus doctor` (Phantom on PATH, unresolved `phm:` refs), so `session_ok: true` is a real verdict on a healthy pin — not a conservative constant.

Cross-process `Mcp-Session-Id` resume is **on** when workers share the same `LOCUS_HOME` (or `LOCUS_MCP_SESSION_DIR`). Multi-tenant remote multiplexor remains open.

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
# initialize mints Mcp-Session-Id (inspect response headers; other POSTs never mint):
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

## Multi-tenant HTTP (`--multi-tenant`)

One `locus-mcp --http --multi-tenant` process (or `LOCUS_MCP_MULTI_TENANT=1`;
stdio + `--multi-tenant` is a startup error, fail closed) serves several
tenants concurrently — each request is routed to a sealed, delegated,
TTL-capped grant session instead of the global pin. `active.json` is never
consulted in this mode, and there is **no ambient fallthrough**: tenantless
requests and stateless provider POSTs are refused outright.

### Operator flow

```bash
# 1. Mint one grant per tenant job (local control only; agents cannot mint).
locus mcp mint --binding cmp --ttl 1h --label "hub-job-42"
# → {"grant_id":"3f2a…","token":"lmt_<grant_id>.<secret>", …}
#   The token is printed exactly once — only its HMAC is stored at rest
#   ($LOCUS_HOME/mcp-grants/<grant_id>.json, 0600).

# 2. Launch the server with the operator control capability in its env
#    (REQUIRED: tenant session validation fails closed for every grant
#    without it — the server warns at startup when it is absent).
#    Mint and serve from the SAME operator-supervised shell/session: grant
#    authority lives in the authority broker, whose lifetime is tied to the
#    supervising process that minted — identical to `locus ci mint`.
LOCUS_MCP_HTTP_TOKEN=… LOCUS_CONTROL_CAPABILITY=… \
  locus-mcp --http 127.0.0.1:8742 --multi-tenant

# 3. Client: BOTH headers on every request — the server token is unchanged
#    admission; the tenant token selects the grant. Never in tool arguments.
curl -s http://127.0.0.1:8742/mcp \
  -H "Authorization: Bearer $LOCUS_MCP_HTTP_TOKEN" \
  -H "X-Locus-Tenant-Token: lmt_<grant_id>.<secret>" \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' -i
# → Mcp-Session-Id minted, bound to the grant, identity pre-anchored.

# 4. Operate / inspect / revoke (CLI-only; tenants can never enumerate
#    each other — there is deliberately NO HTTP enumeration endpoint).
locus mcp list
locus mcp revoke <grant_id>        # or --binding <alias> | --all
```

### Semantics

- **Tenant token on every request** — `X-Locus-Tenant-Token` is required on
  `POST /mcp`, `GET /mcp`, `GET /mcp/sse`, and `DELETE /mcp`. Possession of an
  `Mcp-Session-Id` alone is never authority; revocation propagates on the next
  request because the gate re-reads the grant file every time.
- **Per-session isolation** — catalogs, whoami, drift, resources, prompts,
  `GET /mcp`, and SSE ticks are computed from that grant's session only.
  Agents cannot pin or pick a tenant: `locus_request_pin` returns
  `tenant_fixed_by_grant`.
- **Hard session partition** — MT `Mcp-Session-Id` records live under
  `$LOCUS_HOME/http-sessions-mt/` (or `LOCUS_MCP_SESSION_DIR` + `-mt`), so
  single-tenant servers can never resume tenant records and vice versa.
- **Capacity** — global cap unchanged; per-grant cap
  `LOCUS_MCP_SESSIONS_PER_GRANT` (default 8).
- **Lifetimes** — grant TTL from mint (capped by binding `policy.max_ttl`),
  30m idle HTTP-session TTL (resume with the same token + id while the grant
  lives), explicit `DELETE /mcp` per session, `locus mcp revoke` per grant.
  Worker teardown fires when a grant's last live HTTP session dies.
- **In-flight revoke** — a call that already passed the gate completes its
  single tool call; the next request refuses.

### Failure table

| Response | Meaning |
|----------|---------|
| `401 invalid_grant` | Missing/malformed/unknown/revoked tenant token — deliberately indistinguishable |
| `401 invalid_grant` + `reason: grant_expired` | Token HMAC verified but the grant TTL elapsed; `safe_next` carries the re-mint command |
| `403 tenant_mismatch` | `Mcp-Session-Id` belongs to a different grant (audited `mcp.tenant_mismatch`) |
| `400 session_required` | Stateless non-initialize POST in MT mode |
| tool error `grant_expired` / `grant_revoked` | Grant died mid-session; re-mint (never silent identity fallback) |

Reverse proxies must forward **both** auth headers plus `Mcp-Session-Id`.
Bearer tokens over HTTP make the loopback-refusal/TLS discipline load-bearing:
`LOCUS_MCP_HTTP_ALLOW_REMOTE` semantics are unchanged in MT mode. Audit ops:
`mcp.grant_mint`, `mcp.grant_revoke`, `mcp.grant_auth_fail`,
`mcp.tenant_session_bound`, `mcp.tenant_mismatch`, `mcp.grant_expired_swept`
— all values-free; per-call `mcp.tools_call` rows additionally carry
`grant_id` + `http_session_id`.

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
| `locus_verify_claim` | Verification plane: score a free-text claim (`confidence`, `needs_tool`, `suggestion`) before acting |
| `locus_verify_session` | Hub session pack — same JSON as `locus verify session --json` (`whoami?`, `doctor`, `safe_next`, `session_ok`); runs real external probes (Phantom on PATH, unresolved `phm:` refs) |

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

## MCP auto-pin (advisory only — the server never pins)

Agents **cannot** call pin, and the **server cannot pin on their behalf** either. MCP auto-pin is currently **advisory only**: `locus-mcp` parses the knobs below and runs a once-per-process probe, but pinning requires operator authority — the workspace `.locus.toml` is repo-local (agent-writable) and cannot prove operator intent, and an agent-facing process cannot self-issue session authority. The probe therefore always refuses with `auto-pin requires operator delegation, which is not available: …` and audits **`session.auto_pin_denied`** (with the advisory workspace binding and the refusal reason). A human pins with `locus enter <alias>` / `locus pin <alias>`.

The knobs stay parsed — as the probe's enable/kill signals and so existing client configs keep working unchanged — pending an explicit operator-delegation design. `locus agent setup --apply` still writes `.locus/AGENT.md` with this table and sets `LOCUS_AUTO_PIN=cwd` on MCP client env.

### Probe enable signals (no authority effect)

| Signal | Effect |
|--------|--------|
| `.locus.toml` has `default_binding` | Probe runs at MCP start; the default is recorded as the advisory binding in the denial audit |
| `.locus.toml` has `require_pin = true` | Enables the probe (still needs a resolvable default or autopin target for an advisory binding) |
| `LOCUS_MCP_AUTO_PIN=1` / `true` / `on` | Explicit probe enable |
| `LOCUS_AUTO_PIN=cwd` or `clients.auto_pin = "cwd"` in `$LOCUS_HOME/config.toml` | cwd-based probe enable |

### Kill switches

| Signal | Effect |
|--------|--------|
| `LOCUS_MCP_AUTO_PIN=0` | **Kill switch** — skip the probe entirely (also `false` / `off` / `no`) |
| Omit workspace `default_binding` / `require_pin` and leave enable signals unset | Probe stays off |

Kill switch **wins** over workspace `default_binding` and `LOCUS_AUTO_PIN=cwd`.

Rules:

1. Probe only when **unpinned**; never touches a human pin mid-session.  
2. Only when workspace has `require_pin` or non-empty `default_binding`.  
3. Advisory resolve is read-only — **never `--force`**; workspace `allowed_bindings` always wins.  
4. The pin itself is refused: audits **`session.auto_pin_denied`** (never `session.auto_pin`).  
5. Fail closed: stay unpinned and expose control tools + `locus_request_pin` only.  
6. After a **human** pin/leave, tools / resources / prompts re-read live pin state (`listChanged` advertised; re-read `locus://session` / `locus_context`).

Example workspace (advisory default for humans running `locus enter`, and for the denial audit):

```toml
# .locus.toml
version = 1
default_binding = "acme"
allowed_bindings = ["acme"]
require_pin = true
```

```bash
# kill switch when you want no probe (and no denial audit noise)
export LOCUS_MCP_AUTO_PIN=0
```

## Scope freeze and policy

1. **Policy** (`binding.policy`) may deny or require external authority before the adapter runs. Local `confirm`, approval ids, CLI labels, HTTP tokens, and dashboard actions are advisory only.
2. **Scope freeze** rejects args that conflict with frozen selectors (e.g. another Supabase `project_ref`).  
3. Results are synthetic/identity-oriented in phase 1 — safe to explore without mutating cloud resources.

Agent best practice: call `locus_whoami` or `locus_safe_next` before any infrastructure work. If unpinned or wrong tenant, ask the human to `locus pin <alias>`. When blocked (approval, freeze, doctor), prefer `locus_safe_next` for the single next action.

## Switching clients

```bash
locus switch acme     # one-shot: leave-if-pinned + enter, target validated FIRST
locus pin personal    # human action
# restart or rely on next tools/list — catalog follows the new pin
locus leave           # unbind; agent loses provider tools
```

`locus switch <alias>` validates the target (alias exists, workspace allowlist)
**before** dropping the current pin, so a typo never leaves you unpinned; it
honors `--ttl`/`--force`/`--client` and audits as normal `session.leave` +
`session.pin`.

Workspace defaults (`.locus.toml`) affect CLI pin UX. MCP reads the **active sealed session** under `LOCUS_HOME` only — it never pins the workspace `default_binding` itself (MCP auto-pin is advisory only, see above).

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
| Setup didn’t stick | Re-run `locus agent setup --apply` (re-reads each config after write and fails naming the client+path) or `locus setup --client …` |

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

- [docs/onboarding.md](./onboarding.md) — agency onboarding: 3 agent clients × 3 tenants end-to-end  
- [docs/adapters.md](./adapters.md) — add provider tools  
- [README.md](../README.md) — CLI quick start  
- [CONTRIBUTING.md](../CONTRIBUTING.md) — build and test  
