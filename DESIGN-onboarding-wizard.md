# DESIGN (proposal): guided multi-account binding setup wizard

**Status:** proposal for a future `locus onboard` command. This phase-one
checkout does not contain its CLI implementation. Cargo and npm versions are
still 0.5.0; implementation, verification and a versioned release are separate.

**Proposed defaults (subject to implementation and review):**
- **Ambient import:** detect-only, never auto-import. Every candidate is an
  explicit operator accept/reject; `.mcp.json`/ambient configs become
  *suggestions* that still go through credential-ref wiring (refs only).
  Alternative (full auto-import) rejected: violates "never auto-pin" and
  risks pulling raw tokens.
- **Command tree:** `locus onboard` (top-level, Setup group). Alternatives
  (`locus setup wizard`, `locus init --interactive`) rejected: `locus setup`
  already means MCP client registration.
- **Interactive prompting:** std-only (`std::io` line prompts), no new
  dependency — resolves the supply-chain/MSRV question from the proposal.
- **`--with-samples`:** kept. The wizard is the day-one path for real
  tenants; samples remain the 60-second demo path (`locus quickstart`).
- **Fleet triggering:** local CLI affair in v1. The wizard emits NDJSON
  (`--json`) so Phantom/agent harnesses can drive or audit it
  non-interactively; no deep-link/notification trigger yet.
**Problem:** today, going from `locus init` to a working multi-tenant setup is a
long manual walkthrough ([docs/onboarding.md](./docs/onboarding.md): three
tenants × three agent clients, each binding hand-authored with `--provider`,
`--account`, `--credential-ref`, `--project-ref`…). Operators (and their
agents) must hold the whole binding model in their head before Locus delivers
any value. The "wrong account, impossible" guarantee is only as good as the
bindings people actually create.

## Proposed: `locus setup wizard`

An interactive, re-runnable, TTY-first wizard (non-interactive `--yes` /
`--json` mode for scripts and CI) that walks an operator through binding setup:

1. **Detect.** Probe for ambient identity the wizard can convert: `gh auth
   status` accounts, `~/.aws/config` profiles, Vercel/Supabase/CLIs on PATH,
   existing `.mcp.json` files, `~/.ashlr/config.json`. Present each as a
   candidate — never auto-pin anything.
2. **Name the tenants.** For each candidate, ask: alias (`personal`, `acme`,
   `acme-ro`), tenant label, and role (full vs read-only). Suggest the
   `*-ro` read-only pair for client engagements (firm-mode pattern).
3. **Wire credential refs.** Use `env:VAR` with an explicitly supplied
   provider credential. `phm:NAME` is a compatibility reference, not a working
   Secrets bridge: current Phantom Secrets denies agent-readable reveal, and
   no separately approved credential bridge is active. Never probe values
   through reveal or invent a dry-run reveal command. Bare names, raw tokens, and empty
   refs are rejected at save — the wizard must make it *harder* to paste a
   token than to pick a ref. Unsafe values are never echoed.
4. **Freeze scopes.** Where the provider supports it, capture frozen selectors
   (`project_ref`, `team_id`, `orgs`) and confirm with the operator what
   "denied, not redirected" means for their workflow.
5. **Workspaces.** Offer to drop `.locus.toml` in each repo root (default pin +
   allowlist + `--require-pin`), so `cd`-ing into a client repo can refuse the
   wrong binding automatically.
6. **Verify.** End with `locus doctor` per binding and a `locus exec -- env`
   isolation demo, so the operator *sees* ambient identity disappear.

## Design constraints (non-negotiable)

- **Wizard never holds secrets.** Same as the rest of Locus: refs only; values
  exist transiently in child env during the verify step, never in wizard state,
  logs, or the shell history (disable echo / history for secret prompts).
- **Control capability first.** `init`'s capability bootstrap stays step zero;
  the wizard refuses to run without the control boundary (agents can't drive it).
- **Idempotent & resumable.** State in a wizard plan file under
  `~/.locus/` (refs only); re-running skips completed steps, `--reset`
  starts over.
- **Agent-readable output.** Every step emits a JSON event (`--json`) so
  Phantom/agent harnesses can drive or audit the wizard non-interactively.
- **No new mandatory deps without review.** Interactive prompting would add a
  crate (`dialoguer`/`inquire` class); that's a supply-chain + MSRV decision —
  flag it, don't sneak it in.

## Open questions for Mason

1. Should the wizard *import* ambient MCP configs (`.mcp.json`, Claude
   `settings.json`) automatically, or only ever create Locus-native bindings
   and leave migration to `binding migrate-credential-refs`?
2. Is per-client `--with-samples` still the right day-one artifact, or should
   the wizard replace samples entirely?
3. Where does the wizard live in the command tree — `locus setup wizard`,
   a `locus onboard` alias, or folded into `locus init --interactive`?
4. Should Phantom's fleet be able to *trigger* the wizard on an operator's
   machine (deep link / notification), or is it strictly a local CLI affair?
