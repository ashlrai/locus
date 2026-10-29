# Locus for Phantom-enrolled repos

[Phantom](https://github.com/ashlrai/phantom) is the agentic engineering
workbench. When it enrolls a repo and dispatches agents into it, two
questions must be answered before any provider command runs:

| Question | Owner |
|---|---|
| Can this secret value enter the model's context? | Phantom Secrets (operator-controlled secret supply) |
| Is this command acting as the right GitHub / Vercel / Supabase / Cloudflare account? | **Locus** |

MCP and CLI **account auth** lives in Locus, not in Phantom Secrets. Phantom
Secrets stores the token; Locus decides which token a session may use.

> **Source checkout status: `phm:` resolution is unavailable.**
> Published Locus v0.5.0 has the incompatible legacy reveal adapter. This
> checkout fails closed before invoking Phantom for a `phm:` reference.
> Metadata, pinning and scope checks remain available, but provider execution
> needs supported credentials. See [credential compatibility](./credential-compatibility.md).
> Never capture `phantom reveal` output to work around the missing bridge.
> Use `env:` references and supply values outside agent context.

## 1. One binding per owner scope

Phantom repos belong to one owner scope: `personal`, `company`, or
`client:<name>`. Create one Locus binding per scope and provider. Credential
refs use explicit `env:` names, never raw tokens. Supply their values from an
operator-controlled environment before running provider commands. `phm:`
references remain readable for migration but cannot resolve in this checkout.

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

In each enrolled repo, write a workspace policy so agents can only pin the
scope that owns it:

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
  `locus-mcp` for control tools. For the pinned binding's provider tools,
  register `locus mcp --supervised` from the hooked, pinned shell and restart
  the client after a re-pin. Neither launch returns secret values. Agents can
  request a pin
  (`locus_request_pin`) but cannot pin themselves.
- CI / fleet workers: `locus ci mint|env|run` for short-lived sealed sessions
  that never touch `active.json`.

## 4. Health checks Phantom can call

| Check | Command | Meaning |
|---|---|---|
| Readiness | `locus agent report --json` | `ready` / `protected` / `unsafe` |
| Doctor | `locus doctor --json` | SAFE / WARN / UNSAFE (exit 0/1/2); reports unresolved `phm:` refs |
| Who am I | `locus whoami --json` | the current pin, no secrets |

Phantom should treat `unsafe` or exit 2 as "do not dispatch provider work in
this repo".

## Not in scope today

- Locus does not rotate keys. Rotation stays a Phantom Secrets decision and is
  off by default.
- Locus does not mint deploy tokens. Grants for Phantom-managed deploys must
  be created by a human and referenced as `phm:` names.
