# Locus ↔ Phantom integration sketch

How **Phantom** (AshlrAI's agent fleet orchestrator; answers *can this secret
enter the model?*) plugs into **Locus** (answers *as whom, against which
tenant, right now?*). Written from Locus's side only — nothing here changes
Phantom core.

Full machine contract: [docs/hub-integration.md](./docs/hub-integration.md) ·
drop-in: [integrations/phantom/](./integrations/phantom/) · schemas:
[schema/](./schema/).

---

## The account-separation guarantees

These are the mechanisms that make "wrong account, impossible" literally true,
not a policy you hope the model follows:

1. **One pin, one sealed session.** `locus pin <alias>` seals the session to
   exactly one **binding** (principal × tenant × providers × CredentialRefs ×
   policy) with an HMAC seal plus a supervised live authority broker that binds
   the record digest, backing file, authority, expiry, and a monotonic
   generation. Reading the store file cannot forge or replay authority.
2. **Ambient identity is scrubbed.** `locus exec` / `run` / `ci run` rebuild the
   child environment from a small runtime allowlist (`PATH`, `LANG`, `TERM`,
   …). Everything else — `GH_TOKEN`, `AWS_*`, `SUPABASE_*`, last Vercel team —
   is dropped. Only the pinned binding's providers are re-injected, resolved
   from CredentialRefs (`phm:` via Phantom, `env:` for CI) as standard provider
   env vars, under a private `GH_CONFIG_DIR`/`AWS_*` rooted at
   `~/.locus/workers/<session>/`. Other bindings' providers are never injected.
3. **Frozen scopes.** Provider scopes (`project_ref`, `team_id`, `orgs`) are
   frozen into the binding — an agent call that tries a different selector is
   **denied, not redirected**.
4. **Agents cannot pin.** `locus-mcp` exposes only the pinned binding's tools
   (plus `locus_whoami`) and runs *without* the operator control capability;
   only humans (or audited CI) hold `LOCUS_CONTROL_CAPABILITY`. Agents may only
   `locus_request_pin` — a human approves.
5. **Secrets never surface.** MCP identity/scope responses and `agent report`
   expose credential *presence and source* (`phantom`/`environment`), never
   `credential_ref` values or resolved secrets. Binding files carry refs only —
   bare names, raw tokens, and unsupported schemes are rejected at save/load.
6. **Fail closed.** A `.locus.toml` workspace can restrict which bindings may
   pin in a repo tree; if the file is unreadable or malformed, pins stop and
   `locus doctor` reports `UNSAFE` (exit 2). `watch`/`verify` detect a binding
   changed under a session; `doctor` re-pins. `require_approval` / dual-control
   policy rules block destructive actions pending closed authorization.

## Opt-in: how Phantom uses Locus

- **Shell out, don't reimplement.** All identity ops go through the CLI
  (`locus agent report --json`, `locus ci mint -b <alias> --json`) or the
  `locus-mcp` stdio server. Machine-readable JSON schemas live in `schema/`.
- **Per-job identity:** for each dispatched job, Phantom mints an ephemeral
  sealed session (`locus ci mint -b <alias>`), exports `LOCUS_SESSION_ID` +
  `LOCUS_HOME` into the job's environment, and tears it down after — the
  operator's global pin (`sessions/active.json`) is never touched. The
  drop-in's `withLocusSession(binding, fn)` does exactly this with a scrubbed
  child env.
- **One MCP server named `locus`.** Phantom registers `locus-mcp` (not raw
  `supabase`/`vercel`/`github` MCPs with ambient credentials); the catalog the
  agent sees is the pinned binding's tools only. `required_servers`
  (`locus` + `phantom`) is emitted on every agent report.
- **Pre-mutate gate (opt-in, never always-on):** before dispatching mutating
  work, Phantom runs the fleet preflight
  (`integrations/phantom/fleet-preflight.md`) — gate on `status=unsafe`,
  invalid seal, or `status_oneline ∈ {unpinned, frozen, invalid}` → do not
  dispatch. `LOCUS_ENFORCE=1` enables; default stays off so monorepo CI without
  a pin stays green.

## Opt-out: what changes when Locus is absent

Locus is **additive and off by default**: without a pin, agents run on ambient
identity exactly as they do today — nothing breaks, but the wrong-account
guarantee does not apply. `locus doctor` tells the operator precisely what is
missing (capability, pin, binding) and how to fix it. The only hard coupling is
the `REQUIRED_SERVERS` convention: fleets that want the guarantee must register
the `locus` MCP server instead of ambient provider MCPs.

## Failure modes (what breaks, and how loudly)

| Failure | Behavior |
|---|---|
| Session seal invalid / binding changed under session | `status`/`verify` fails → `UNSAFE`; pre-mutate gate denies dispatch |
| Workspace `.locus.toml` unreadable or malformed | `pin`/`enter` refuse; `doctor` → `UNSAFE` (fail closed) |
| Pin for a binding outside the workspace allowlist | refused unless `--force` (audited) |
| Credential ref can't resolve at spawn | `--no-resolve` preflight fails before child/worker/session effects; locator names never logged |
| Agent hits `require_approval` / `dual_control` | MCP returns `appr_…`; provider execution stays blocked until a closed external authorization envelope |
| MCP HTTP token unset/wrong | loopback HTTP MCP refuses; stdio path unaffected |
| Phantom itself down | `phm:` refs fail to resolve → isolated env simply lacks those vars (no ambient fallthrough) |
