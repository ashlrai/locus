# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **`locus quickstart`** — first-60s bootstrap: config + samples if empty, enter workspace default / sample pin, whoami + doctor verdict
- **`locus completion <shell>`** — bash/zsh/fish/elvish/powershell via clap_complete
- **locus-mcp AI-native surface**
  - Resources: `locus://session`, `locus://doctor`, `locus://bindings` (`resources/list` + `resources/read`)
  - Prompt: `locus_context` system fragment (`prompts/list` + `prompts/get`)
  - Tool descriptions tagged `[locus:<alias|unpinned>]`; `locus_whoami` always first in `tools/list`
  - `initialize.instructions` agent rules; capabilities advertise tools + resources + prompts
  - **MCP auto-pin** from workspace `default_binding` / `require_pin`, or `LOCUS_MCP_AUTO_PIN=1` / `LOCUS_AUTO_PIN=cwd` / `clients.auto_pin=cwd` — once per process, audit `session.auto_pin`, never force allowlist (`LOCUS_MCP_AUTO_PIN=0` kill switch)
  - Protocol tests for resources, prompts, description tags, and auto-pin

### Changed

- **`locus init`** — writes `config.toml` with `notify.enabled = false` when missing; AI-native next steps (`setup` · `enter` · `doctor`); annotated sample bindings
- **Shell hook** — prompt shows `[locus:FROZEN]`, `[locus:enter!]` when unpinned in `require_pin` workspaces; status oneline token `require_pin`
- Landing page / agent docs: prefer `locus enter` before tool use; v0.1.1 graph + CI commands

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
- `locus exec` — scrub ambient identity env, resolve CredentialRefs into child only
- Private worker dirs under `~/.locus/workers/<session>/` (GH/AWS config isolation)
- Shell hooks: `locus hook zsh|bash|fish`
- Credential refs: `phm:NAME`, `env:VAR`, `test:VALUE` (test only with `LOCUS_ALLOW_TEST_CREDS=1`)
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
- Dual-control: `policy.dual_control` / `dual_control_all_approvals` — two distinct principals
- Approval store under `~/.locus/approvals/{id}.json` (args never stored raw — `args_digest` only)
- Stable `approval_id` (`appr_<24 hex>`); grant via `locus approve grant <id> --as <principal>`
- Canonical `args_digest`: key-order independent, nested secret keys stripped
- Grant TTL (default 15m); deny is terminal; expired grants re-block

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

[Unreleased]: https://github.com/ashlrai/locus/compare/v0.1.1...HEAD
[0.1.1]: https://github.com/ashlrai/locus/releases/tag/v0.1.1
[0.1.0]: https://github.com/ashlrai/locus/releases/tag/v0.1.0
