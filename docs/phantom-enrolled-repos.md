# Locus for Phantom-enrolled repos

[Phantom](https://github.com/ashlrai/phantom) is the agentic engineering
workbench. When it enrolls a repo and dispatches agents into it, two
questions must be answered before any provider command runs:

| Question | Owner |
|---|---|
| Can this secret value enter the model's context? | Phantom Secrets (proxy and placeholder controls) |
| Is this command acting as the right GitHub / Vercel / Supabase / Cloudflare account? | **Locus** |

MCP and CLI **account auth** lives in Locus, not in Phantom Secrets. Phantom
Secrets can protect model credentials; Locus scopes provider credentials for a session.

> **Current status:** `phm:` references are retained as metadata but cannot
> resolve credentials. Locus fails closed before a credential-resolving CLI
> launch and does not invoke `phantom reveal`. Use `env:` references for
> provider work, with values supplied outside agent context. See
> [credential compatibility](credential-compatibility.md). The examples below
> use `env:` references; never place their values in binding files or chat.

## 1. One binding per owner scope

Phantom repos belong to one owner scope: `personal`, `company`, or
`client:<name>`. Create one Locus binding per scope and provider. Credential
refs are environment variable names, never raw tokens. Supply each value outside agent context.

```bash
locus binding add personal --tenant personal --provider github \
  --account masonwyatt23 --credential-ref env:GH_TOKEN_PERSONAL --non-interactive
locus binding add ashlr --tenant ashlr --provider github \
  --account ashlrai --credential-ref env:GH_TOKEN_ASHLR --org ashlrai --non-interactive
locus binding add client-acme --tenant acme --provider vercel \
  --account acme-prod --credential-ref env:VERCEL_TOKEN_ACME --team-id team_xxx --non-interactive
```

Use `--dry-run` first to print the binding TOML without writing it.

## 2. Pin the repo to its scope

In each enrolled repo, write a workspace policy limiting pins to the
scope that owns it. A human performs pin changes:

```bash
cd ~/code/phantom            && locus workspace --default ashlr --allow ashlr --require-pin
cd ~/code/clients/acme-site  && locus workspace --default client-acme --allow client-acme --require-pin
```

This writes `.locus.toml`. Commit it: it holds aliases only, no secrets.
A malformed or unreadable `.locus.toml` fails closed.

## 3. Run Phantom's agents inside the binding

- One-shot commands: `locus run -b ashlr -- gh pr list` (global pin unchanged).
- Interactive agent sessions: `locus pin` in the repo, then start the agent.
- MCP clients: `locus setup --client claude|cursor|codex|grok|gemini` wires
  `locus-mcp` with control tools only. Register `locus mcp --supervised`
  to expose provider tools, and launch the client from the shell that entered
  the pin. Restart the client after re-pinning. MCP results never return secret
  values. Agents can request a pin (`locus_request_pin`) but cannot pin.
- CI / fleet workers: `locus ci mint|env|run` for short-lived sealed sessions
  that never touch `active.json`.

## 4. Health checks Phantom can call

| Check | Command | Meaning |
|---|---|---|
| Readiness | `locus agent report --json` | `ready` / `protected` / `unsafe` |
| Doctor | `locus doctor --json` | SAFE / WARN / UNSAFE; advisory-only WARN can exit 0, other WARN exits 1, UNSAFE exits 2; reports unresolved `phm:` refs |
| Who am I | `locus whoami --json` | the current pin, no secrets |

Phantom should treat `unsafe` or exit 2 as "do not dispatch provider work in
this repo".

## Not in scope today

- Locus does not rotate keys. Rotation stays a Phantom Secrets decision and is
  off by default.
- Locus does not mint deploy tokens. Grants for Phantom-managed deploys must
  be created by a human and supplied through supported `env:` references.
