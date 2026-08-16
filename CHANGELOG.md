# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **Signed adapter-registry release manifests** — `locus adapter registry
  export` emits a canonical JSON snapshot of the built-in adapter set
  (`locus-adapter-registry/v1`: id, name, crate version, sorted tools, and a
  `sha256` digest over each entry's canonical material). `--sign` signs it
  with an operator-provided ed25519 key (`--key <file>` or
  `LOCUS_REGISTRY_SIGNING_KEY`; base64 or 64-hex seed, never generated or
  printed; `--sign` without a key refuses to export). `locus adapter
  verify-manifest <file>` is fail-closed: the signature must verify against
  the existing trust store (`$LOCUS_HOME/trust/adapter-keys.toml` +
  `LOCUS_ADAPTER_TRUST_KEYS`) **and** the running binary's adapter set must
  match the manifest exactly (version, ids, names, tools, digests);
  `--allow-unsigned` permits only a *missing* signature for drift-only checks.
  Release CI attaches the unsigned canonical manifest as
  `locus-adapters-<tag>.json` (operators sign locally; a commented
  `TODO(registry-signing)` block documents future CI signing). Docs:
  `docs/registry-trust.md`.
- **Hub multi-tenant drop-in** — `integrations/ashlr-hub/locus.ts` gains
  `withLocusMcpTenant(binding, fn)` (mint → dispatch with
  `X-Locus-Tenant-Token` headers → always-revoke, token held in memory only),
  pure `parseMcpMintOutput` / `parseMcpListOutput` /
  `classifyTenantAuthError` helpers, and async `locusMtPreflight()`;
  `schema/mcp-grant.schema.json` documents the mint/list contract.
- **MT conformance coverage** — `scripts/e2e.sh` step 31 exercises the full
  two-tenant lifecycle (mint, isolation, cross-tenant 403, revoke → 401);
  `scripts/dogfood.sh` gains an opt-in `DOGFOOD_MT=1` probe;
  `scripts/hub-smoke.sh` / `hub-integration-test.sh` cover the grant CLI and
  live drop-in slice.

### Changed

- **HTTP auth is Bearer-only in `Authorization`** — `locus-mcp --http` now
  accepts the shared token as `Authorization: Bearer <token>` (or the
  `X-Locus-Token` / `X-Locus-MCP-Token` headers) only. The undocumented raw
  schemeless `Authorization: <token>` fallback accepted through v0.4.0 is
  rejected with `401`. Migration: prefix the header value with `Bearer `.
- **stdio frame cap** — the stdio MCP transport enforces the same 8 MB
  Content-Length cap as HTTP; an oversized frame is refused before allocation
  and the server exits with a protocol error (fail closed) instead of
  attempting an unbounded read.
- **Catalog deny annotation** — `tools/list` descriptions of tools a
  structured policy `deny` rule unconditionally blocks under the current pin
  now carry `[denied by policy under current pin]`, computed by the same
  policy engine as the call gate (companion to the existing
  `[requires human approval under current pin]` marker). `policy.default =
  "deny"` is deliberately not annotated.
- **Watch heartbeat control-capability findings** — `locus watch` ticks now
  attach the operator-shell `LOCUS_CONTROL_CAPABILITY` readiness findings that
  `locus doctor` reports, so hub heartbeat consumers see the same degraded
  posture (`session_ok` reflects the escalated verdict).

## [0.4.0] — 2026-08-16

### Added

- **Multi-tenant MCP multiplexor (`locus-mcp --http --multi-tenant`)** — one
  HTTP process serves several tenants concurrently via operator-minted grants
  (`locus mcp mint|list|revoke`, local control boundary only; agents cannot
  mint). Each grant wraps a sealed, delegated, TTL-capped session
  (`active.json` is never consulted — no ambient fallthrough) and a
  `lmt_<grant_id>.<secret>` bearer token whose HMAC-only record lives at
  `$LOCUS_HOME/mcp-grants/<grant_id>.json` (0600); the token itself is printed
  exactly once at mint. `X-Locus-Tenant-Token` is required on every `/mcp`,
  `/mcp/sse`, and `DELETE` request on top of the unchanged
  `LOCUS_MCP_HTTP_TOKEN`; the grant file is re-read per request so revocation
  propagates within one call. Fail-closed responses: uniform `401
  invalid_grant` (malformed/unknown/revoked indistinguishable; `grant_expired`
  + re-mint `safe_next` only after HMAC proof), `403 tenant_mismatch`
  (audited), `400 session_required` for stateless POSTs. Catalogs, whoami,
  drift, resources, prompts, `GET /mcp`, and SSE ticks are computed per grant;
  `locus_request_pin` / `locus_enter_hint` return `tenant_fixed_by_grant`;
  tenants cannot enumerate each other (grant listing is CLI-only). Tenant
  `Mcp-Session-Id` records live in a hard `http-sessions-mt/` partition with a
  per-grant cap (`LOCUS_MCP_SESSIONS_PER_GRANT`, default 8) and worker
  teardown when a grant's last session dies. Identity is pre-anchored from
  the grant at session mint (zero wrong-account window). New values-free
  audits: `mcp.grant_mint`/`grant_revoke`/`grant_auth_fail`/
  `tenant_session_bound`/`tenant_mismatch`/`grant_expired_swept`;
  `mcp.tools_call` rows additionally carry `grant_id` + `http_session_id`.
  Doctor gains a values-free `mcp_multi_tenant` section (active grants per
  alias, expired-unswept warning). Stdio + `--multi-tenant` is a startup
  error. Docs: [docs/mcp.md](./docs/mcp.md#multi-tenant-http---multi-tenant),
  [docs/hub-integration.md](./docs/hub-integration.md).
- **Anthropic + OpenAI provider adapters** — synthetic freeze-check tools
  (`anthropic.scope|whoami|usage|keys.list|keys.create`, same set for
  `openai.*`) for per-tenant model-API spend isolation. Org id freezes via
  `scope.account_id`, workspace/project id via `scope.project_ref`;
  provider-native selector spellings (`org`, `organization_id`, `workspace_id`,
  `project_id`, + camelCase) are freeze-netted; `keys.create` is destructive
  (hidden + denied under `read_only`). Matrix:
  [docs/adapters.md](./docs/adapters.md).
- **`locus switch <alias>`** — one-shot leave-if-pinned + enter with
  pre-flight target validation (alias suggestions + workspace allowlist
  checked **before** dropping the current pin), honoring
  `--ttl`/`--force`/`--client`, compact identity block + `--json`; audits as
  normal `session.leave` + `session.pin`.

### Fixed

- `locus leave --force` audits success only after `active.json` removal
  actually succeeds; a failed removal now writes a
  `session.force_leave_failed` audit record with the error instead of a
  false success trail.
- Claude registration probe also detects **local-scope** registrations in
  `~/.claude.json` under `projects.<cwd>.mcpServers.locus` (literal and
  canonicalized project keys); `ClaudeMcpScope` gains an additive `local`
  variant and `both` now means more-than-one-scope.

### Security

- Worker-home deletion hardened (`leave`, `leave --force`, CI/grant session
  cleanup): the recorded path and `$LOCUS_HOME/workers/` are both
  canonicalized and strict containment is required — `..`-traversal and
  symlink escapes are refused, and the workers root itself is never removed.

## [0.3.0] — 2026-08-15

Dogfood polish and release hygiene on top of 0.2.0. No identity-plane
invariant weakens — same fail-closed pin, freeze, scrub, and exclusive catalog,
now also anchored per MCP session (`pin_changed`).

### Added

- **Auto-leave TTL on `enter`/`pin` (`--ttl`)** — `locus enter <alias> --ttl 30m`
  requests a shorter pin (min 1m, max 24h), riding the existing sealed
  `expires_at` + passive fail-closed expiry check (no timer). Precedence:
  `--ttl` flag > new optional binding `policy.default_ttl` > `policy.max_ttl`,
  always capped by `max_ttl` (silent clamp + CLI warning). `default_ttl` is
  excluded from the binding fingerprint so tuning it never freezes a live
  session. Surfacing: expiry line with local time + remaining on enter/pin,
  additive `expires_in_secs` on `whoami` and the doctor `pin` slice,
  `* pinned (… left)` in `binding list`, a `pin_expiring` Warn doctor finding
  under 5 minutes remaining, and `ttl_secs`/`ttl_source` on the `session.pin`
  audit event.
- **Guided client onboarding (`locus client add`)** — interactive front door
  over the `binding add` primitive (which is now flags-first with prompts only
  for missing values on a TTY; `--guided`, `--non-interactive`, `--dry-run`).
  Prompts reuse existing validators: alias charset + reserved `locus*` prefix +
  exists/did-you-mean, provider menu from new `known_providers()`, per-provider
  scope fields, and `CredentialRef::validate` so a pasted raw secret is never
  accepted (bare Phantom names get a `phm:NAME` suggestion). Writes through the
  existing `Store::save_binding` path (validation, lock, audit) and ends with
  next steps (`enter --ttl`, `agent setup --apply`, `doctor`, `phantom add`).
  New optional flags: `--account-id`, `--repos`, `--default-ttl`. Guide:
  [docs/onboarding.md](./docs/onboarding.md).
- **Fail-closed MCP session pin-anchoring (`pin_changed`)** — every MCP session
  (stdio process / HTTP `Mcp-Session-Id`) anchors to the *identity* of the
  binding observed at initialize (or first healthy pinned observation):
  primary `binding_id` + tenant + mode + sorted `(alias, binding_id)`
  namespace pairs — never the session id. A **cross-alias re-pin under a live
  MCP session now returns a structured `pin_changed` tool error** (anchored vs
  current identity + `safe_next.action=reinitialize_client`) instead of
  silently adopting the new identity, and it outranks the
  `runtime_unhealthy`/`executor_authority_unavailable` noise from the staled
  executor grant. Same-alias re-pin (TTL refresh) is unaffected at the anchor
  layer (a stdio process whose executor capability was granted for the old
  session may still need a client restart — authority-plane fact, not masked).
  The refusal is session-local: it never mutates or freezes `active.json`.
  Recovery: re-initialize/restart the client to adopt the new pin (HTTP: POST
  `initialize` with the existing `Mcp-Session-Id` re-anchors in place), or
  `locus enter <anchored>` to restore. Control tools
  (whoami/status/heartbeat/safe_next/verify_session) gain an additive
  `mcp_anchor` block (omitted when no anchor exists); `locus_verify_session`
  forces `session_ok=false` on mismatch so hub gating fails closed per
  session; `tools/list` collapses to control tools tagged with the anchored
  alias; `GET /mcp` reports `anchor_ok` when a session id is presented. The
  anchor survives `locus leave` (not_pinned refusals gain anchor context),
  `LOCUS_SESSION_ID` overlay sessions are structurally immune, and the HTTP
  session disk format gains an optional serde-default `anchor` field (still
  v1 — old binaries ignore it, old records adopt-once; enforced across
  process restarts from disk). New audits: `mcp.anchor_established`,
  `mcp.anchor_repin`, `mcp.anchor_reset`, `mcp.anchor_mismatch` (deduped;
  aliases/tenants/binding_ids/session_ids only). Sessionless HTTP POSTs
  (no `Mcp-Session-Id`) share a **process-level anchor** (same decide()
  machinery, stdio parity) so stateless provider `tools/call` is pin-swap
  protected too; a fresh sessionless `initialize` re-anchors it (adoption
  path). Optional `LOCUS_MCP_HTTP_REQUIRE_SESSION=1` still refuses
  sessionless provider `tools/call` outright (default off — stateless CI
  POSTs keep working, anchored at process scope).
- **Grok Build write path + Claude user scope** — `locus setup --client grok`
  and `locus agent setup --apply --client grok` now write Grok Build's
  documented `~/.grok/config.toml` (`[mcp_servers.locus]`, Codex-style TOML)
  with the same fail-closed `toml_edit` merge as Codex (unparseable ⇒ abort
  untouched); grok joins `--client all` and the registration probe
  (`mcp_registered.grok`) reads `~/.grok/config.toml` by default
  (`LOCUS_GROK_MCP_CONFIG` still overrides; JSON or TOML);
  `scripts/dogfood-clients.sh` detects Grok. `--client generic`
  stays print-only for clients without a known config path. New
  `locus agent setup --claude-scope user|project` (default `project`):
  user scope registers for all projects via the claude CLI
  (`claude mcp add-json locus … --scope user`, verified with
  `claude mcp get locus`) — `~/.claude.json` is never hand-edited, and setup
  fails closed with instructions when the `claude` CLI is absent. JSON server
  entries for Claude/Cursor now carry `"type": "stdio"` (documented as
  required by Cursor; harmless elsewhere).

- **`locus_verify_session` MCP tool (M5)** — hub session pack over MCP, same JSON as
  `locus verify session --json` (`{ kind: "session", version, whoami?, doctor, safe_next,
  session_ok }`). Available unpinned; gate on `session_ok`; `isError` only on hard store
  failures. Values-free — aliases, verdicts, scopes only. Audit keys
  (`session_ok` / `safe_next` / `doctor_verdict` / `doctor_ok` / `has_whoami`) aligned with
  the CLI. Docs: [docs/mcp.md](./docs/mcp.md), [docs/hub-integration.md](./docs/hub-integration.md).
- **Control-capability onboarding** — `locus init` / `locus quickstart` mint and persist
  `LOCUS_CONTROL_CAPABILITY` (`$LOCUS_HOME/control_capability`, mode `0600`, never
  overwritten, value never printed); `locus hook zsh|bash|fish` exports it when the shell
  lacks one; `locus doctor` flags missing / not-exported / invalid / mismatched capability
  with the exact fix. Docs: README quick start, [docs/mcp.md](./docs/mcp.md),
  [docs/agency-starter.md](./docs/agency-starter.md).
- **Control-capability posture management** — persistence stays the fresh-store
  default; strict operators opt out with `--no-persist-capability` on
  `init` / `quickstart` (mint to process env only, print the export line, never
  write the file) and manage posture with the new
  `locus capability status|persist|unpersist` subcommand (`status` never prints
  the bearer; `persist` writes 0600, idempotent, fail-closed on mismatch;
  `unpersist` removes the file and prints the export line). New doctor severity
  **Info** — can never escalate the verdict — carries the INFO finding
  `control_capability_persisted` whenever a valid capability file exists, with
  the exact strict-posture fix. Docs: new "Control-plane authority boundary"
  section in [SECURITY.md](./SECURITY.md) (the capability file is ambient for
  same-user processes, `LOCUS_HOME` is same-user-readable regardless, and the
  MCP surface — not the CLI — is the agent boundary).
- **Post-write verification in `locus agent setup --apply`** — re-reads every client
  config it wrote via the registration probe and exits 1 naming each failing client+path;
  warns when the `locus-mcp` path falls back to a bare name; explicit `--mcp-bin` must exist.
- **Alias ergonomics** — `enter` / `pin` unknown-alias errors list known aliases with a
  did-you-mean suggestion; `binding list` marks the pinned alias; the `locus*` alias prefix
  is reserved at create/import (unroutable through the MCP gate), with a doctor warn for
  legacy bindings.
- **Hub drop-in watch/verify session heartbeat** — port pure + shell helpers from hub
  [PR #273](https://github.com/ashlrai/ashlr-hub/pull/273) into
  [`integrations/ashlr-hub/locus.ts`](./integrations/ashlr-hub/locus.ts):
  `parseWatchHeartbeat`, `parseSessionVerificationPack`, `locusVerifySession`,
  `locusWatchOnce`, `locusSoftWatchHeartbeat` (soft annotation under `LOCUS_ENFORCE=warn`
  only — never a hard blocker alone). Firm onboard soft-offer remains hub CLI-only (#274).
  Docs: [docs/hub-integration.md](./docs/hub-integration.md),
  [docs/verification-plane.md](./docs/verification-plane.md).
- **HTTP `Mcp-Session-Id` file-backed resume (M5)** — in-memory cache plus disk map under
  `$LOCUS_HOME/http-sessions/` (override `LOCUS_MCP_SESSION_DIR`). Atomic write, idle TTL
  prune, fail-closed corrupt files; stores id + timestamps + optional pin summary only
  (never secrets). Restarts / multi-worker resume via load-on-miss. Docs: [docs/mcp.md](./docs/mcp.md).

- **Opt-in worker sandbox network deny** — `LOCUS_WORKER_SANDBOX_NO_NETWORK=1` or
  `upstream.sandbox_no_network = true` applies harder isolation without changing the MCP
  default (network still **allowed** unless opted in). Linux `bwrap` gets `--unshare-net`;
  macOS Seatbelt omits outbound allows (best-effort). The Linux `path` backend fails closed
  when no-network is requested. Docs: [docs/workers.md](./docs/workers.md).
- **Multi-message SSE on `locus-mcp --http` (M5 partial)** — when Accept prefers
  `text/event-stream` (or lists SSE and the JSON-RPC body is large), POST `/mcp`
  streams multiple `event: message` frames: `locus.sse.progress` (+ optional
  `locus.sse.chunk` text slices) then the complete JSON-RPC result. Header
  `X-Locus-Streamable: sse-multi|sse-single`. Env: `LOCUS_MCP_SSE_MULTI_BYTES`,
  `LOCUS_MCP_SSE_CHUNK_BYTES`.
- **`GET /mcp/sse` session heartbeat** — token-auth SSE stream of values-free
  `locus.session_tick` (`session_ok`, doctor verdict, safe_next, pin alias).
  Query `?once=1` / `?interval=5s`; env `LOCUS_MCP_SSE_INTERVAL`. Fail-closed auth.
  Hub alternative to CLI `locus watch` over HTTP.
- Docs: [`docs/mcp.md`](./docs/mcp.md) multi-SSE + session stream; GOALS M5 partial updated.
- Tests: multi-SSE tools/list, Accept dual upgrade, `/mcp/sse` auth + once tick.

- **Linux worker sandbox partial (M5)** — `LOCUS_WORKER_SANDBOX=1` / `upstream.sandbox`
  on Linux prefers bubblewrap when installed (`LOCUS_WORKER_SANDBOX_BACKEND=bwrap`:
  RO system roots, bind work tree + session worker home only, shared network for MCP,
  no `~/.locus/bindings` bind). Falls back to best-effort `path` (restricted PATH +
  absolute executable only — **not** kernel isolation; tag is explicit). macOS Seatbelt
  unchanged. Docs: [docs/workers.md](./docs/workers.md). Not a full seccomp/VM.
- **Streamable-HTTP-lite MCP (M5 partial)** — `locus-mcp --http` gains session-sized
  polish toward remote multiplexor without a full SSE rewrite:
  - `GET /mcp` (token) — capabilities + pin summary + tool **names** only (values-free)
  - Accept negotiation: prefer `application/json`; single-event `text/event-stream`
    when Accept is SSE-only; **406** when Accept allows neither; **415** on non-JSON body
  - `GET /health` advertises `transport: streamable-http-lite` + endpoint map
  - Remote deploy notes in [`docs/mcp.md`](./docs/mcp.md) (reverse proxy, token,
    `LOCUS_HOME`, pin-before-serve)
  - Tests: health, auth fail-closed on GET/POST `/mcp`, capabilities shape, SSE-lite, 406/415
- **`locus watch` session heartbeat (M5)** — each tick runs the same pack as
  `locus verify session` (doctor + whoami + safe_next + `session_ok`), not drift alone.
  - `--json`: one NDJSON object per tick
    `{ kind:"watch", session_ok, whoami?, doctor_verdict, safe_next, pinned, frozen }`
  - `--require-ok`: fail closed whenever `session_ok` is false (hub / CI)
  - `--once` without `--require-ok`: non-zero only when a pin was present/expected and not ok
  - Docs: [docs/verification-plane.md](./docs/verification-plane.md)
- **Audit webhook sink (M5 partial)** — `locus events export --sink webhook [--url URL]`
  posts redacted fleet-pulse JSONL or OTLP JSON to a remote URL for SIEM / log shippers.
  Env: `LOCUS_AUDIT_WEBHOOK_URL`. **Fail soft** when URL unset (skip, exit 0);
  **fail closed** if export body matches secret patterns (refuse POST). See
  [`docs/observability.md`](./docs/observability.md).
- **`scripts/dogfood.sh`** — after forensics export, also prints **`locus goal status`**
  (from repo `GOALS.md` when present) and runs **`scripts/hub-smoke.sh`** (own throwaway
  home; skip with `DOGFOOD_SKIP_HUB_SMOKE=1`)
- Shell e2e feature-detects post-0.2 surfaces:
  - **`locus verify claim --text … --json`** (URL claims → `needs_tool`)
  - MCP **`locus_safe_next`** unpinned (`action=enter`) + pinned (no secrets)
  - **`locus upstream list --json`** (recipe catalog)
  - Optional **`locus watch --once --json`** heartbeat shape (no secrets)
  - Re-asserts **notify disabled by default** after the full suite
- Landing page ([`apps/web/`](./apps/web/)) — verify claim + `locus_safe_next` callouts
  alongside dashboard / forensics / HTTP MCP / goal status

### Changed

- **MCP auto-pin is now honestly advisory-only (`locus-mcp`)** — the server
  never pins. The once-per-process probe resolves the workspace target
  read-only and audits **`session.auto_pin_denied`** (advisory binding +
  operator-delegation refusal reason) instead of minting authority;
  `pin_auto_delegated` fails closed with a clear "auto-pin requires operator
  delegation, which is not available" error (workspace `.locus.toml` defaults
  are advisory hints only — an agent/MCP process cannot self-issue session
  authority). `LOCUS_AUTO_PIN` / `LOCUS_MCP_AUTO_PIN` knobs stay parsed as
  probe enable / kill switches pending an operator-delegation design. All
  advertising surfaces (tool/resource descriptions, `.locus/AGENT.md`, setup
  notes, [docs/mcp.md](./docs/mcp.md)) now say the server never pins; the
  session-anchor layer anchors only after a human pin.
- **Docs: hub #277 firm soft-warn + goals sync** — document ashlr-hub
  [PR #277](https://github.com/ashlrai/ashlr-hub/pull/277) firm fleet soft-warn
  (doctor/readiness only: enrolled&gt;0 + locus available + `locus.firm` false →
  non-blocking `locus-firm` warn; **never hard-blocks mutate**; monorepo firm
  default stays off). Hub-only `checkLocusFirm` contract sketch; do **not** port
  into [`integrations/ashlr-hub/locus.ts`](./integrations/ashlr-hub/locus.ts).
  GOALS M4 marks #277 landed; always-on firm default remains open. GOALS M5 notes
  cross-process HTTP session resume landed ([#31](https://github.com/ashlrai/locus/pull/31));
  still open only multi-tenant remote multiplexor. Docs:
  [GOALS.md](./GOALS.md), [docs/hub-integration.md](./docs/hub-integration.md),
  [integrations/ashlr-hub/doctor-check.md](./integrations/ashlr-hub/doctor-check.md),
  [integrations/ashlr-hub/README.md](./integrations/ashlr-hub/README.md),
  [integrations/ashlr-hub/fleet-preflight.md](./integrations/ashlr-hub/fleet-preflight.md).

### Fixed

- **MCP anchor health surfaces use the gate's identity comparison** —
  `mcp_anchor` / `anchor_ok` / `locus_verify_session` overrides now compare
  the full anchored identity (binding_id + tenant + **mode + namespace
  triples**) when a full observation is available, primary-only just when only
  the drift identity exists — so `session_ok=false` whenever provider tools
  would refuse with `pin_changed` (a mode/namespace re-pin no longer reads
  healthy while tools are wedged).
- **Multi-worker HTTP anchor writes are last-writer-safe** — the per-request
  session touch and cross-worker observations adopt the on-disk anchor
  (authoritative) before persisting, so one worker's initialize-time reset or
  just-persisted establishment is never clobbered by a sibling's stale
  in-memory anchor. Anchor values are only authored by
  establish/repin/initialize-reset paths.
- **Claude user-scope registration is add-first** — `locus agent setup
  --claude-scope user` no longer removes the existing `locus` entry before
  adding: a failing add leaves any previous registration intact (and says
  so); only a CLI "already exists" refusal triggers remove + re-add, and a
  failure after that removal is reported honestly (entry removed, currently
  unregistered) instead of claiming nothing changed.
- **TOML paste output escapes interpolated values** — `[mcp_servers.locus]`
  snippets (`agent setup --client generic|grok`, `setup --print`)
  basic-string-escape the `locus-mcp` path and env values, so quotes /
  backslashes in the binary path can no longer emit invalid TOML.
- **Phantom deadline runner cannot leak blocked reader threads** — the child
  runs in its own process group and the deadline kill takes out grandchildren
  holding the stdout pipe (wrapper shells); the reader thread is reaped with
  a bounded join, and a hard cap on live helper threads fails closed instead
  of accumulating leaks against a repeatedly wedged `phantom`.
- **Doctor reports an honest authority-anchor verification status** — new
  additive `pin.authority_anchor_verified` field: `false` (with `seal_ok`
  still `true` and the existing `executor_authority_unavailable` Warn
  finding, never a false UNSAFE) when the anchor check was skipped because no
  control capability was present; `true` only when the check actually passed.
- **MCP doctor packs run real external probes** — `locus_verify_session`, the
  `locus://doctor` resource, and `GET /mcp/sse` session ticks now share core
  `gather_doctor_external` (Phantom on PATH + unresolved `phm:` refs — provider/source
  metadata only, locator names never leak) instead of a hardcoded conservative failure.
  `session_ok` can be `true` on a healthy pin and MCP output matches
  `locus verify session --json`.
- **Fail-closed MCP config merges** — a malformed `.mcp.json` / `~/.cursor/mcp.json` is
  left byte-for-byte unchanged with a remediation error (previously it was silently
  replaced, destroying other registered servers); the Codex merge is now a
  format-preserving `toml_edit` upsert of `[mcp_servers.locus]` that heals stale command
  paths and missing env tables, preserves other tables/comments, and cannot produce
  duplicate keys.
- **`locus setup` parity with `agent setup`** — writes the same agent env
  (`LOCUS_AUTO_PIN=cwd`, `LOCUS_CLIENT`) instead of an empty `env: {}`; the codex arm
  actually merges `~/.codex/config.toml` (`--print` emits the full `[mcp_servers.locus]`
  + env TOML); quickstart/doctor hints standardize on `locus agent setup --apply`.
- **Authority broker start under CI load** — production handoff wait raised from 3s → 10s;
  optional `LOCUS_AUTHORITY_BROKER_START_TIMEOUT_MS` override; timeout errors may include a
  short broker stderr snip. `scripts/hub-smoke.sh` retries init/pin and defaults the
  override to 15s so hub contract smoke stays resilient after heavy test jobs (flaky
  "broker startup timed out" unrelated to webhook export).

### Tests

- Core: webhook POST against local `TcpListener`, secret-scan fail-closed, URL resolve
  (explicit / env / blank → soft skip)
- Shell e2e (`scripts/e2e.sh`): prior 0.2 surface **plus** verify / safe_next / upstream /
  watch heartbeat / notify late re-check — **44+ checks** on full current command set
- CLI unit tests: watch interval parse, fail policy, heartbeat from pinned/unpinned packs
- Core unit `inv_notify_default_false` + e2e step 15/27 keep desktop notifications **OFF**
  by default under clean `LOCUS_HOME`
- Composite worker tests for top-adapter recipe expansion (`github-mcp`,
  `supabase-mcp`, `vercel-mcp` sandbox gates), exclusive synthetic catalog +
  scope freeze alongside recipe-shaped upstream, and ambient provider-secret
  exclusion when `resolve_secrets = false`

### Security

- **HTTP pre-auth hardening (`locus-mcp --http`)** — request caps enforced before any body
  allocation or the token check: **413** above 8 MB `Content-Length`, **431** above 32 KB
  header bytes or 128 header fields; an unparseable JSON-RPC body returns **400** with
  JSON-RPC `-32700` instead of a dropped connection; `Mcp-Session-Id` is minted **only on
  `initialize`** (other POSTs without the header are served statelessly), so garbage POSTs
  can no longer exhaust session capacity.
- **Scope freeze covers provider-native selector spellings** — the upstream-worker
  preflight now freezes camelCase and provider-native aliases
  (`projectRef`/`project_id`/`projectId`, `teamId`, `accountId`, `aws_account_id`/`awsAccountId`,
  `stripe_account`/`stripeAccount`) alongside the canonical snake_case keys at any depth
  inside object **and array** args (bounded scan — args nesting past the limit deny fail
  closed); non-string values under a frozen selector key and Stripe `livemode` flips are
  denied at any depth too; `org`/`owner`/`organization` args must be members of the frozen
  `scope.orgs`; tools whose
  provider prefix is not declared on the pinned binding are explicitly denied (fail closed).
- Local CLI/dashboard approval assertions are now explicit `local_advisory` evidence and can no longer unlock provider execution or satisfy dual control. Stdio MCP, HTTP MCP, dashboard, doctor, and forensics expose the disabled external-authority state; gated calls fail closed until an independent cryptographic verifier exists.
- Caller-controlled principal strings, Touch ID mocks, dashboard mutations, and unsigned same-user JSON never count as human authority. Provider execution remains blocked until a peer-authenticated OS broker verifies a non-agent-accessible issue capability.

## [0.2.0] — 2026-08-09

AI-native / hub-native identity plane polish on top of 0.1.1. Local dashboard,
forensics packs, HTTP MCP, goal loop, agent report contract, verification stubs,
upstream recipes, and faster doctor probes. **Wrong account, still impossible.**

### Added

#### Local identity dashboard

- **`locus serve [--port 8750] [--token] [--open]`** — loopback-only Axum server (UI + JSON API)
- **`locus dashboard`** — same server + open browser (`--no-open` to skip)
- API: `GET /api/health|status|whoami|bindings|approvals|doctor|events`, `POST /api/approve/{id}/grant`
- Security: bind `127.0.0.1` only; no resolved secrets; optional `LOCUS_DASHBOARD_TOKEN`
- UI: [`apps/dashboard/`](./apps/dashboard/) embedded in the CLI binary

#### Forensics & observability

- **`locus forensics export [--binding] [--out pack.json]`** — shareable pack: pin/session meta, binding summaries, audit tail, doctor snapshot, pending approvals, near-miss, chain tip (**no secrets**)
- **`locus events export [--otlp] [--out file]`** — fleet-pulse JSON lines or OTLP Logs JSON; [`docs/observability.md`](./docs/observability.md)
- Doctor **`near_miss_count` / `near_miss`** — scope_freeze + require_approval blocks in the last 24h

#### AI-native agent surface

- **`locus agent setup|doctor|report`** — wire MCP clients, readiness ladder, hub JSON contract
- **locus-mcp resources** — `locus://session`, `locus://doctor`, `locus://bindings`
- **Prompt** — `locus_context` system fragment (`prompts/list` + `prompts/get`)
- Tool descriptions tagged `[locus:<alias|unpinned>]`; `locus_whoami` always first
- `initialize.instructions` agent rules; capabilities advertise tools + resources + prompts
- **MCP auto-pin** from workspace / `LOCUS_AUTO_PIN=cwd` / `LOCUS_MCP_AUTO_PIN` (kill switch `=0`); audit `session.auto_pin`; never force allowlist
- **`locus_safe_next`** — single best next action (`enter` / `re_pin` / `approve` / `doctor_fix` / `ready`)
- **HTTP MCP** — `locus-mcp --http 127.0.0.1:8742` or `LOCUS_MCP_HTTP=1`; `POST /mcp` + `GET /health`; requires `LOCUS_MCP_HTTP_TOKEN`; loopback by default

#### Verification plane (M5 stubs)

- **`locus verify claim --text "…"`** — heuristic claim scoring (`confidence`, `needs_tool`, `suggestion`, `signals`, optional pin grounding)
- MCP **`locus_verify_claim`** — same shape (available unpinned)
- Doctor may WARN on recent low-confidence claim signals; [`docs/verification-plane.md`](./docs/verification-plane.md)

#### Upstream recipes

- **`locus upstream list`** / **`locus upstream suggest <provider>`**
- Binding TOML: `upstream = { recipe = "github-mcp", resolve_secrets = true }` (`adapters/recipes.toml`)
- Recipes: `github-mcp`, `github-official`, `supabase-mcp`, `filesystem-mcp`, `everything-mcp`

#### Goal loop & hub composition

- **`locus goal status [--json]`** — northstar progress from [`GOALS.md`](./GOALS.md) or embedded milestones
- **[GOALS.md](./GOALS.md)** — vision, goal tree, milestone checklist, success metrics
- Hub drop-in [`integrations/ashlr-hub/`](./integrations/ashlr-hub/) — `locus.ts` (attested parent-broker gate, env-free `withLocusSession`, `registerLocusInMcpConfig`, pure parse helpers), gateway snippet, doctor-check, [`fleet-preflight.md`](./integrations/ashlr-hub/fleet-preflight.md)
- Schemas: [`schema/agent-report.schema.json`](./schema/agent-report.schema.json), [`schema/doctor.schema.json`](./schema/doctor.schema.json), [`schema/hub-gate.schema.json`](./schema/hub-gate.schema.json)
- [`scripts/hub-smoke.sh`](./scripts/hub-smoke.sh) + [`scripts/hub-integration-test.sh`](./scripts/hub-integration-test.sh) composition smoke

#### DX

- **`locus quickstart`** — first-60s bootstrap: samples → enter → whoami + doctor
- **`locus completion <shell>`** — bash/zsh/fish/elvish/powershell
- **`locus topic <name>`** / **`locus help topic <name>`** — product guides (`dashboard`, `forensics`, `serve`, `goal`, `verify`, `agent`, `mcp`, `http`, `upstream`)
- **Adapter SDK** — [`docs/adapter-sdk.md`](./docs/adapter-sdk.md), `examples/adapters/_template/`, `adapters/manifest.toml`

### Changed

- **`phantom --version` process-cached** — doctor, agent report, forensics, and dashboard share one probe per process (faster hot paths)
- Dashboard `/api/doctor` skips deep `phantom list` inventory (CLI doctor still full-checks)
- **`locus init`** — `notify.enabled = false` by default; AI-native next steps; annotated samples
- Shell hook — `[locus:FROZEN]`, `[locus:enter!]` / `require_pin` oneline token
- CLI help regrouped: Setup · Daily use · CI · Approvals · Audit · Local UI · Maintenance
- Landing page ([`apps/web/`](./apps/web/)) — AI-native identity plane hero, dashboard / forensics / HTTP MCP / goal status
- Packaging version-aligned at **0.2.0** (Cargo workspace, npm `locus-cli` / `locus-mcp`, homebrew formula mirror)

### Tests

- Shell e2e (`scripts/e2e.sh`): prior 0.1.1 surface **plus** feature-detected dashboard `/api/health`, `forensics export`, `goal status`, `topic` — **38 checks** (0 skipped) on full 0.2 command set
- MCP HTTP transport tests; agent report / forensics pack schema keys; protocol resources/prompts/auto-pin

### Security

- Same fail-closed invariants as 0.1.x (seal, exclusive catalog, scrub, scope freeze, agents cannot pin)
- Dashboard and HTTP MCP bind loopback by default; token required for MCP HTTP; no resolved secrets in API or packs
- Forensics / events export / agent report expose credential presence/source metadata only, never secret values or locator names.
- Bindings require explicit supported credential refs (`phm:` or `env:`); release binaries reject `test:` regardless of environment, and bare/raw/unsupported values fail closed without being echoed in errors.
- MCP, CI, CLI, audit, worker, dashboard, and child-process surfaces never expose locator names or provider stderr.
- Malformed, unreadable, broken-link, or non-file nearest `.locus.toml` blocks pin/autopin/run (including forced pin), and doctor reports `UNSAFE`.
- Legacy bare Phantom names are surfaced as invalid and can be converted only through explicit `binding migrate-credential-refs ... --write`; unsafe values require manual repair.
- Credential-ref migration uses a locked, fsynced intent/completion journal and no-clobber staged replacement; exact retries reconcile crashes, concurrent replacements, and pending audit writes without locator disclosure.
- Hub ephemeral-session helpers scrub ambient credentials and accept only validated identity/scope metadata from `ci mint`; the dogfood gate now requires the same strict `ready` dispatch contract as Fleet preflight.

## [0.1.1] — 2026-08-09

Firm UX polish on top of the initial public release. Notifications stay quiet by
default; doctor is a single mission-control pane; run/ns cover one-shot and
experimental multi-bind workflows; local encrypted graph share + CI mint land.

### Added

#### Firm UX

- **`locus enter` / `leave`** — firm workflow pin + clear identity
- **`locus engagement init|close`** — client engagement lifecycle (metadata; never deletes vault secrets)
- **Autopin** — default binding from workspace / config when pin has no alias
- **`locus run -b <alias> -- <cmd>`** — one-shot child session; global pin unchanged (`--share-pin` opt-in)
- **`locus pin --ns a,b`** — experimental namespaced multi-binding; MCP tools appear as `alias__tool`
- **`locus watch`** — drift freeze when the pinned binding changes under a live session
- Structured policy rules + improved `approve wait` / grant UX

#### Doctor single pane

- **`locus doctor`** — home/seal/bindings, active pin (alias/tenant/expires/seal), runtime drift, pending approvals + dual-control waiting, phantom + unresolved phm refs, autopin/`config.toml`, workspace allowlist, last 5 audit ops
- Verdict **SAFE | WARN | UNSAFE** (exit **0 / 1 / 2**); stable mission-control JSON schema
- **`locus events --last N [--op] [--binding] [--json]`** — audit JSONL tail with filters
- MCP **`locus_heartbeat`** — agent-safe drift/runtime summary (never secrets)

#### Graph & CI

- **`locus graph list|export|import`** — encrypted local binding-graph share (bindings + workspace templates, CredentialRefs only; `LOCUS_GRAPH_PASSPHRASE`)
- **`locus ci mint|env|run`** — short-lived sealed sessions under `sessions/ci-*.json` without mutating `active.json`; children set `LOCUS_SESSION_ID`

#### Agency kit & docs

- Agency starter kit — [`examples/agency-starter/`](./examples/agency-starter/) + [`docs/agency-starter.md`](./docs/agency-starter.md)
- [docs/agency-certainty.md](./docs/agency-certainty.md) — identity vs epistemic certainty (Ashlr stack)
- Firm-mode docs refreshed for enter/run/notify/doctor daily path

#### Packaging

- Release workflow still publishes `locus-<triple>.tar.gz` for aarch64/x86_64 Darwin + x86_64 Linux
- Homebrew formula mirror + npm wrappers (`locus-cli`, `locus-mcp`) version-aligned at **0.1.1**

### Changed

- **Desktop approval notifications are OFF by default** — no spam, no sound; opt in with `locus notify on` or `LOCUS_NOTIFY=1`; kill switch `locus notify off` / `LOCUS_QUIET=1` / `CI=true`; silent + rate-limited when enabled
- CLI help regrouped: Setup · Daily use · CI · Approvals · Audit · Maintenance

### Tests

- Shell e2e (`scripts/e2e.sh`): pin/isolation/MCP freeze/approval, dual-control two-principal grant, doctor exit codes, events, enter/run, notify off-by-default, graph export/import, `ci mint`, `locus_heartbeat` — all **feature-detected** where optional — **34 checks** (0 skipped) on full 0.1.1 command set
- Core unit coverage for doctor verdicts, namespaced tool prefixing, notify defaults, CI session isolation

### Security

- Same fail-closed invariants as 0.1.0 (seal, exclusive catalog, scrub ambient identity, scope freeze)
- Notify path is best-effort UX only — never surfaces errors on the agent/tool path
- Graph export never includes secret values (refs + templates only); encrypted at rest with passphrase
- Namespaced multi-bind remains explicit opt-in (`--ns`); exclusive pin stays the default isolation model
- CI mint does not rewrite the human shell pin (`active.json`)

## [0.1.0] — 2026-08-06

Initial public release of **Locus** — identity plane for coding agents.
**Wrong account, impossible.**

### Added

#### Control plane (daemon-less)

- Workspace crates: `locus-core`, `locus-cli` (`locus`), `locus-mcp`
- Binding store under `~/.locus` (overridable via `LOCUS_HOME`)
- CLI: `locus init`, `pin`, `leave`, `whoami`, `status`, `binding list|show|add|rm`
- HMAC-sealed session pins (tampering fails closed; seal verified on privileged paths)
- Workspace `.locus.toml` — default binding, allowlist, `require-pin`
- `locus exec --no-resolve` — manual identity diagnostics only; provider credentials stay inside policy-gated MCP workers
- Private worker dirs under `~/.locus/workers/<session>/` (GH/AWS config isolation)
- Shell hooks: `locus hook zsh|bash|fish`
- Credential refs: `phm:NAME` and `env:VAR`; production binaries always reject `test:`
- Continuous identity check: `Store::verify_runtime` / drift surface (seal, binding id, tenant, expiry)
- Path-safe binding/approval ids (`validate_name_component`, `validate_approval_id`)

#### MCP multiplexor

- `locus-mcp` stdio server (JSON-RPC 2.0 / MCP subset; NDJSON + Content-Length)
- Control tools: `locus_whoami`, `locus_status`, `locus_list_bindings`, `locus_request_pin`, `locus_providers` (when pinned)
- Agents **cannot** pin — `locus_request_pin` returns instructions only
- Unpinned session ⇒ control tools only; seal verified on tools/list and tools/call
- `locus setup --client claude|cursor|codex` for MCP config merge
- `locus doctor` readiness checks

#### Provider adapters & scope freeze

Synthetic identity/scope tools with hard freeze on account selectors:

| Provider    | Freeze knobs                                      |
|-------------|---------------------------------------------------|
| Supabase    | `project_ref`, `read_only`                        |
| GitHub      | orgs / repos surface                              |
| Vercel      | `team_id`, projects, env                          |
| Cloudflare  | `account_id`                                      |
| AWS         | `account_id`, profile (extra)                     |
| Stripe      | `account_id`, `livemode` (bool freeze)            |
| Resend      | domain allowlist                                  |

- Model-supplied selector mismatch → error (not warn)
- Generic `{provider}.scope` for unknown providers

#### Policy & human approvals

- Policy defaults: `allow` / `deny` + `require_approval` globs
- Dual-control policy declarations (external authority adapter still required)
- Approval store under `~/.locus/approvals/{id}.json` (args never stored raw — `args_digest` only)
- Stable `approval_id` (`appr_<24 hex>`); local advisory evidence via `locus approve grant <id> --as <label>`
- Canonical `args_digest`: key-order independent, nested secret keys stripped
- Advisory records remain pending; deny is terminal; authoritative TTL path is disabled

#### Workers & upstream MCP

- Synthetic worker backend (in-process adapters)
- MCP stdio worker: spawn upstream MCP with isolated env, handshake, tools/call fan-out
- Per-provider `upstream` in binding TOML (`command`, `args`, `resolve_secrets`)
- Composite worker manager (synthetic + optional upstream per provider)
- Example: [`examples/upstream.binding.toml`](./examples/upstream.binding.toml)

#### Packaging & docs

- Open-source: CI, release workflow, CONTRIBUTING, SECURITY, CODE_OF_CONDUCT
- Adapter / MCP / workers docs under `docs/`
- Homebrew formula mirror: `integrations/homebrew`
- npm wrappers: `locus-cli`, `locus-mcp` (download release binary or cargo fallback)
- Sample bindings: `examples/acme.binding.toml`, `personal.binding.toml`, `workspace.locus.toml`

#### Hardening tests

- Property-style `args_digest` tests (key order, nested objects, secret strip)
- Adapter freeze: Cloudflare `account_id`, Stripe `livemode`, AWS `account_id`
- Pin/leave stress (many sequential cycles)
- Invalid seal after leave/re-pin + recover
- Binding validate: empty providers, bad alias, incomplete provider, malformed TOML
- Isolation integration + MCP protocol freeze-deny paths

### Security

- Session seal verification on MCP tool list/call paths
- Unbound session ⇒ control tools only
- MCP never returns secret values
- Workspace allowlist + optional `--force` for out-of-allowlist pins
- Approval id path traversal rejected
- Ambient CLI identity scrubbed on `locus exec` / worker spawn

### Known limitations

- Most adapters remain identity/scope stubs; live upstream fan-out depends on per-binding `upstream` config
- Team binding graph / multi-namespace sessions — later (see [PLAN.md](./PLAN.md)); experimental `--ns` is local-only
- Homebrew source `sha256` must be refreshed after each tag (see [docs/RELEASE.md](./docs/RELEASE.md))

[Unreleased]: https://github.com/ashlrai/locus/compare/v0.4.0...HEAD
[0.4.0]: https://github.com/ashlrai/locus/releases/tag/v0.4.0
[0.3.0]: https://github.com/ashlrai/locus/releases/tag/v0.3.0
[0.2.0]: https://github.com/ashlrai/locus/releases/tag/v0.2.0
[0.1.1]: https://github.com/ashlrai/locus/releases/tag/v0.1.1
[0.1.0]: https://github.com/ashlrai/locus/releases/tag/v0.1.0
