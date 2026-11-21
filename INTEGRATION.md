# Locus ↔ Phantom integration sketch

<!-- fresh:begin versions -->
| Component | Version |
|---|---|
| Rust workspace (source) | 0.7.2 |
| npm locus-cli (published) | 0.7.0 |
| npm @ashlrai/locus-mcp (published) | 0.7.0 |
| Published release date | 2026-11-06 |
<!-- fresh:end -->

How the **Phantom workbench** (AshlrAI's agent fleet orchestrator) can compose
with **Locus** (answers *as whom, against which
tenant, right now?*). Written from Locus's side only — nothing here changes
Phantom core.

Full machine contract: [docs/hub-integration.md](./docs/hub-integration.md) ·
drop-in: [integrations/phantom/](./integrations/phantom/) · schemas:
[schema/](./schema/).

---

## The account-separation guarantees

These mechanisms apply to dispatch through an authorized Locus session.
They do not constrain unrelated processes outside that session:

1. **One pin, one sealed session.** `locus pin <alias>` seals the session to
   exactly one **binding** (principal × tenant × providers × CredentialRefs ×
   policy) with an HMAC seal plus a supervised live authority broker that binds
   the record digest, backing file, authority, expiry, and a monotonic
   generation. Reading the store file cannot forge or replay authority.
2. **Ambient identity is scrubbed.** `locus exec` / `run` / `ci run` rebuild the
   child environment from a small runtime allowlist (`PATH`, `LANG`, `TERM`,
   …). Everything else — `GH_TOKEN`, `AWS_*`, `SUPABASE_*`, last Vercel team —
   is dropped. Only the pinned binding's providers are re-injected, resolved
   from supported `env:` CredentialRefs as standard provider env vars.
   `phm:` is a compatibility reference, not an active Secrets bridge: current
   Secrets denies agent-readable reveal, so a `phm:` ref stops a
   credential-resolving launch before the child or upstream worker starts. An
   unresolved `env:` ref is never filled from ambient env: the child starts
   without it and the issue is reported, unless `--strict-creds` stops the
   launch (a credential-resolving MCP worker always refuses). `HOME`,
   `GH_CONFIG_DIR` and the `AWS_CONFIG_FILE`/`AWS_SHARED_CREDENTIALS_FILE`
   paths point into the session's private worker home, so the user's global
   CLI config is never read. Other bindings' providers are never injected.
3. **Frozen scopes.** Provider scopes (`project_ref`, `team_id`, `orgs`) are
   frozen into the binding — an agent call that tries a different selector is
   **denied, not redirected**.
4. **Agents cannot pin.** `locus-mcp` serves the `locus_*` control tools and,
   only when launched as `locus mcp --supervised` (or under `locus exec`), the
   pinned binding's provider tools. It runs *without* the operator control
   capability;
   only humans (or audited CI) hold `LOCUS_CONTROL_CAPABILITY`. Agents may only
   `locus_request_pin` — a human approves.
5. **Secrets never surface.** MCP identity/scope responses and `agent report`
   expose credential *presence and source* (`phantom`/`environment`), never
   `credential_ref` values or resolved secrets. Binding files carry refs only —
   bare names, raw tokens, and unsupported schemes are rejected at save/load.
6. **Fail closed.** A `.locus.toml` workspace can restrict which bindings may
   pin in a repo tree; if the file is unreadable or malformed, pins stop and
   `locus doctor` reports `UNSAFE` (exit 2). `watch`/`verify` detect a binding
   changed under a session; `doctor` only reports findings and never re-pins.
   `require_approval` / dual-control policy rules block destructive actions
   pending closed authorization.

## Opt-in: how Phantom uses Locus

- **Shell out, don't reimplement.** All identity ops go through the CLI
  (`locus agent report --json`, `locus ci mint -b <alias> --json`) or the
  `locus-mcp` stdio server. Machine-readable JSON schemas live in `schema/`.
- **Proposed per-job identity:** an explicitly configured workbench can mint an ephemeral
  sealed session (`locus ci mint -b <alias>`), export `LOCUS_SESSION_ID` +
  `LOCUS_HOME` into the job's environment, and tear it down after — the
  operator's global pin (`sessions/active.json`) is never touched. The
  drop-in's `withLocusSession(binding, fn)` does exactly this with a scrubbed
  child env.
- **One MCP server named `locus`.** Phantom registers Locus (not raw
  `supabase`/`vercel`/`github` MCPs with ambient credentials). A bare
  `locus-mcp` entry serves control tools only; register
  `locus mcp --supervised` to expose the pinned binding's provider tools (see
  [docs/mcp.md](./docs/mcp.md)). `required_servers`
  (`locus` + `phantom`) is emitted on every agent report.
- **Pre-mutate gate (opt-in, never always-on):** before dispatching mutating
  work, Phantom runs the fleet preflight
  (`integrations/phantom/fleet-preflight.md`) — dispatch only when the agent
  report has `status=ready`, `ready=true` and a healthy `alias:tenant`
  `status_oneline`; `unpinned`, `require_pin`, `frozen`, `invalid` or an
  unknown token blocks. `LOCUS_ENFORCE=1` enforces and `LOCUS_ENFORCE=warn`
  only logs; with the env unset, the workbench config's `locus.enforce` then
  `locus.firm` decide, and the default stays off so monorepo CI without a pin
  stays green.

## Opt-out: what changes when Locus is absent

A workbench may leave Locus integration disabled, in which case Locus offers
no identity guarantee for its unrelated processes. Through Locus itself, an
unbound session exposes control tools only and cannot fall through to ambient
provider credentials. `locus doctor` tells the operator precisely what is
missing (capability, pin, binding) and how to fix it. The only hard coupling is
the `REQUIRED_SERVERS` convention: fleets that want the guarantee must register
the `locus` MCP server instead of ambient provider MCPs.

## Failure modes (what breaks, and how loudly)

| Failure | Behavior |
|---|---|
| Session seal invalid / binding changed under session | `status`/`verify` fails → `UNSAFE`; pre-mutate gate denies dispatch |
| Workspace `.locus.toml` unreadable or malformed | `pin`/`enter` refuse, even with `--force`; `doctor` → `UNSAFE` (fail closed) |
| Pin for a binding outside the workspace allowlist | refused unless `--force` (audited) |
| Credential ref cannot resolve | No ambient fallback; locator names never logged. MCP stdio workers refuse to launch; `exec`/`run`/`ci run` report the issue and start the child without that credential unless `--strict-creds` |
| Agent hits `require_approval` / `dual_control` | MCP returns `appr_…`; provider execution stays blocked until a closed external authorization envelope |
| MCP HTTP token unset/wrong | `locus-mcp --http` will not start without `LOCUS_MCP_HTTP_TOKEN`; a wrong token gets `401 unauthorized`; stdio path unaffected |
| `phm:` reference with current Phantom Secrets | Unsupported credential integration; refuse dispatch rather than start a child without its required credentials |
