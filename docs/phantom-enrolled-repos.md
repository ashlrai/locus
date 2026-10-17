# Locus for Phantom-enrolled repos

[Phantom](https://github.com/ashlrai/phantom) is the agentic engineering
workbench. When it enrolls a repo and dispatches agents into it, two
questions must be answered before any provider command runs:

| Question | Owner |
|---|---|
| Can this secret value enter the model's context? | Phantom Secrets (`phm:` refs, `phantom reveal` only into child env) |
| Is this command acting as the right GitHub / Vercel / Supabase / Cloudflare account? | **Locus** |

MCP and CLI **account auth** lives in Locus, not in Phantom Secrets. Phantom
Secrets stores the token; Locus decides which token a session may use.

> **Current status: `phm:` refs do not resolve with Phantom Secrets 0.7.9.**
> Locus resolves `phm:NAME` by running `phantom reveal --yes NAME`, and
> Phantom Secrets 0.7.9 refuses `--yes` (plaintext reveal needs a trusted
> interactive terminal). A `phm:` binding still pins and lists its provider
> tools, but `locus exec` / `locus run` start the worker with the provider
> variables (`GH_TOKEN`, ...) unset and print
> `warn unresolved credentials: provider=github source=phantom code=unavailable`;
> `--strict-creds` fails before the child runs, and `locus doctor` reports
> `WARN` with `[unresolved_phm]`. Do not work around this by capturing
> `phantom reveal` output. Until a reviewed Phantom/Locus handoff ships, use
> `env:` refs for provider work and supply the value from your
> operator-controlled environment:
>
> ```bash
> locus binding add personal --tenant personal --provider github \
>   --account masonwyatt23 --credential-ref env:LOCUS_PERSONAL_GH_TOKEN --non-interactive
> export LOCUS_PERSONAL_GH_TOKEN=...   # from your own secret store, outside agent context
> locus pin personal && locus exec -- gh auth status
> ```
>
> The `phm:` steps below describe the intended composition and keep working
> for pinning, scope freeze and doctor checks.

## 1. One binding per owner scope

Phantom repos belong to one owner scope: `personal`, `company`, or
`client:<name>`. Create one Locus binding per scope and provider. Credential
refs are Phantom names, never raw tokens.

```bash
phantom add GH_TOKEN_PERSONAL      # value prompted, never echoed
phantom add GH_TOKEN_ASHLR
phantom add GH_TOKEN_ACME

locus binding add personal --tenant personal --provider github \
  --account masonwyatt23 --credential-ref phm:GH_TOKEN_PERSONAL --non-interactive
locus binding add ashlr --tenant ashlr --provider github \
  --account ashlrai --credential-ref phm:GH_TOKEN_ASHLR --org ashlrai --non-interactive
locus binding add client-acme --tenant acme --provider vercel \
  --account acme-prod --credential-ref phm:VERCEL_TOKEN_ACME --team-id team_xxx --non-interactive
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
  `locus-mcp`; it exposes only the pinned binding's tools and never returns
  credential refs or secret values. Agents can request a pin
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
