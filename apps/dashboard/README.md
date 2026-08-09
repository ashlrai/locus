# Locus Identity Dashboard

Local mission-control UI for the Locus identity plane.

## Quick start

```bash
# Build CLI, then launch UI + API on 127.0.0.1:8750 and open a browser
cargo run -p locus-cli -- dashboard

# API only (no browser)
cargo run -p locus-cli -- serve --port 8750
```

Optional shared-secret (recommended if other local users share the machine):

```bash
export LOCUS_DASHBOARD_TOKEN=dev-only-token
locus dashboard
```

## Endpoints

All bind **127.0.0.1 only**. Responses never include resolved secrets — only aliases, digests, scopes, and counts.

| Method | Path | Notes |
|--------|------|--------|
| GET | `/` | Static dashboard |
| GET | `/api/status` | Active pin summary |
| GET | `/api/whoami` | Whoami (CredentialRefs only) |
| GET | `/api/bindings` | Binding list |
| GET | `/api/approvals` | Pending approvals |
| GET | `/api/doctor` | Full doctor report |
| GET | `/api/events?last=N` | Audit tail |
| POST | `/api/approve/{id}/grant` | Body: `{"principal":"…","ttl":"15m?"}` |

If `LOCUS_DASHBOARD_TOKEN` (or `--token`) is set, every `/api/*` call requires:

- `Authorization: Bearer <token>`, or
- `X-Locus-Token: <token>`, or
- `?token=<token>`

## Static UI

`public/index.html` is embedded into the `locus` binary at compile time and served by `locus serve` / `locus dashboard`. Edit the HTML and rebuild to pick up changes.

Design language matches [`apps/web`](../web/) (dark terminal grid, mono labels, SAFE/WARN/UNSAFE).

## Security

- Loopback only (`127.0.0.1`)
- No credential resolution
- Optional token gate
- Agents cannot re-pin from the dashboard (grant only; pin stays human CLI)
