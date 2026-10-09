# DESIGN (proposal): community adapter marketplace

**Status:** proposal for future adapter discovery and installation commands.
This phase-one checkout does not contain the marketplace implementation.
Cargo and npm versions remain 0.5.0; this document does not announce a release.

**Proposed defaults (subject to implementation and review):**
- **Index model:** registry-agnostic from day one. Any HTTPS URL can serve a
  static index JSON (`locus adapter registry index add <url>`); no
  AshlrAI-run canonical index is hardcoded. AshlrAI may publish a *curated*
  index later — it would be one source among many, distinguished by the
  `ashlrai-curated` publisher signature, not by code.
- **Review bar:** publisher-signed-only for community adapters. Trust =
  the operator pins the publisher's key (`locus adapter trust add`); there
  is no central review gate in v1. An AshlrAI-curated tier reuses the same
  machinery with AshlrAI's signature as the trust anchor.
- **Scope:** strictly provider adapters in v1. Skills/prompts/recipes are
  not covered (different distribution + trust questions).
- **Monetization:** none — the marketplace is an ecosystem accelerant for
  Phantom/Locus adoption.
**Problem:** Locus ships 9 built-in provider adapters (github, supabase,
vercel, aws, cloudflare, stripe, resend, anthropic, openai) with an exportable
registry manifest and a trust store (`locus adapter registry export`,
`locus adapter verify-manifest`, `locus adapter trust`). The long tail of
providers (Linear, Notion, Salesforce, Snowflake, …) can't all be built-in —
but the *trust model for third-party adapters doesn't exist yet*. Today an
adapter is either compiled in (trusted by construction) or it doesn't exist.
Release catalogs are unsigned; local signing and operator trust are separate
explicit actions. Archive checksum sidecars do not sign an adapter catalog.

## Proposed: signed community adapter registry

Build on the existing registry trust machinery rather than inventing a new one:

1. **Adapter manifest as the distribution unit.** A community adapter is a
   versioned manifest (id, provider name, tool surface, env-key mapping,
   scope selectors, signature) in the same canonical JSON the built-in
   registry already exports. Adapters are *declarative config*, not code —
   the MCP stdio worker machinery (`crates/locus-core/src/workers/`) already
   spawns and scopes them, so no new execution primitive is needed.
2. **Publisher signatures, operator trust.** Publishers sign manifests with
   ed25519; operators add publisher keys via `locus adapter trust add`
   (per-publisher, per-adapter, or per-version pinning). `verify-manifest`
   stays fail-closed: unsigned or wrong-signer manifests are refused, and the
   running binary's adapter set must match the manifest exactly (as today).
3. **Discovery without ambient trust.** `locus adapter search` / `locus
   adapter install <id>` against a registry index (static JSON over HTTPS to
   start — no new service required). Install = download manifest → verify
   signature against trust store → record in `~/.locus/adapters/`. Updating
   never silently widens tool surface: a manifest whose tool list grows
   requires explicit re-approval.
4. **Namespace the risk.** Community adapters run through the same isolation
   pipeline as built-ins: isolated env, scope freeze, `require_approval`
   policy, and MCP surfaces limited to the pinned binding. A malicious adapter
   signature authenticates its publisher; it does not sandbox upstream code.
   An upstream MCP command still executes code and may use the binding's
   credentials, so approving its command and scope remains necessary.

## Explicitly out of scope (v1)

- Executing third-party *code* (WASM/native plugins). Declarative manifests
  only — code execution is a different threat model and a different design doc.
- A hosted registry service with accounts/billing. Static signed index first;
  the trust model must not depend on a server being honest.
- Auto-update. Adapters update only on explicit operator action, with a diff
  of the tool surface shown before approval.

## Open questions for Mason

1. Should AshlrAI run the canonical index, or should the format be
   registry-agnostic from day one (any HTTPS URL as an index source)?
2. What's the review bar for "community" adapters — signed-by-publisher only,
   or an AshlrAI-curated tier with our signature as well?
3. Does the marketplace also cover *skills/prompts/recipes* (MCP resources +
   prompts), or strictly provider adapters?
4. Monetization shape, if any — or is the marketplace purely an ecosystem
   accelerant for Phantom/Locus adoption?
