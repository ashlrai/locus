# Locus Identity Dashboard

Local mission-control UI for the Locus identity plane.

## Quick start

```bash
# Build CLI, then launch UI + API on 127.0.0.1:8750 and open a browser
cargo run -p locus-cli -- dashboard

# Serve and print the URL without opening a browser
cargo run -p locus-cli -- dashboard --no-open

# API only (no browser)
cargo run -p locus-cli -- serve --port 8750
curl -s http://127.0.0.1:8750/api/health
```

`locus serve --swarm <graph.json>` is a different mode: it serves a grant-scoped MCP broker instead of this dashboard (see [docs/swarm-broker.md](../../docs/swarm-broker.md)).

Optional shared-secret (recommended if other local users share the machine):

```bash
export LOCUS_DASHBOARD_TOKEN=dev-only-token
locus dashboard
```

## Endpoints

All bind **127.0.0.1 only**. Responses never include resolved secrets — only aliases, digests, scopes, and counts. Dashboard approval labels are advisory and cannot enable `locus exec`, `locus run`, `locus ci run`, or provider execution.

| Method | Path | Notes |
|--------|------|--------|
| GET | `/`, `/index.html` | Static dashboard |
| GET | `/api/health` | Liveness: `ok`, `service`, `version`, `bind`. Never token-gated |
| GET | `/api/status` | Active pin summary |
| GET | `/api/whoami` | Whoami (CredentialRefs only) |
| GET | `/api/bindings` | Binding list |
| GET | `/api/approvals` | Pending approvals |
| GET | `/api/doctor` | Full doctor report |
| GET | `/api/events?last=N&op=OP&binding=ALIAS` | Audit tail. `last` defaults to 50 and is clamped to 1–500; `op` matches exactly or as a substring (e.g. `scope_freeze`); `binding` matches exactly |
| POST | `/api/approve/{id}/grant` | JSON body `{"principal": "...", "ttl": "..."}` (`ttl` optional). Records an untrusted local advisory label; never authorizes provider execution |

If `LOCUS_DASHBOARD_TOKEN` (or `--token`) is set, every `/api/*` call except `/api/health` requires:

- `Authorization: Bearer <token>`, or
- `X-Locus-Token: <token>`, or
- `?token=<token>`

`locus dashboard` / `locus serve --open` open the plain URL, never `?token=`, because the browser opener gets the URL as a process argument (visible in `ps`) and the browser keeps it in history. With a token set, the page asks for it once and keeps it in that browser.

## Static UI

`public/index.html` is embedded into the `locus` binary at compile time and served by `locus serve` / `locus dashboard`. Edit the HTML and rebuild to pick up changes.

Design language matches [`apps/web`](../web/) (dark terminal grid, mono labels, SAFE/WARN/UNSAFE).

## Security

- Loopback only (`127.0.0.1`)
- No credential resolution
- Optional token gate
- Agents cannot re-pin from the dashboard.
- Dashboard assertions are `local_advisory`; a bearer/dashboard token is not human identity and cannot satisfy approval or dual-control authority.
- External cryptographic approval authority is not implemented, so gated provider execution remains fail-closed.
- `/api/status.capabilities` reports each CLI/MCP surface separately: `exec` reflects the live active-pin state, while `run` and `ci run` report their explicit-binding requirement. All three can inject credentials into their child by default. Their shared `--no-resolve` guard expands recipe defaults, rejects resolving upstreams before effects, and permits credential-free upstreams. The UI renders missing capability data as `unknown / degraded` rather than inferring a security assurance.
