# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

Dogfood polish and release hygiene on top of 0.2.0. No identity-plane
invariants change — same fail-closed pin, freeze, scrub, and exclusive catalog.

### Fixed

- **Authority broker start under CI load** — production handoff wait raised from 3s → 10s;
  optional `LOCUS_AUTHORITY_BROKER_START_TIMEOUT_MS` override; timeout errors may include a
  short broker stderr snip. `scripts/hub-smoke.sh` retries init/pin and defaults the
  override to 15s so hub contract smoke stays resilient after heavy test jobs (flaky
  "broker startup timed out" unrelated to webhook export).

### Added

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

[Unreleased]: https://github.com/ashlrai/locus/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/ashlrai/locus/releases/tag/v0.2.0
[0.1.1]: https://github.com/ashlrai/locus/releases/tag/v0.1.1
[0.1.0]: https://github.com/ashlrai/locus/releases/tag/v0.1.0
