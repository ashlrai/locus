# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
- Team binding graph / multi-namespace sessions — later (see [PLAN.md](./PLAN.md))
- Homebrew `sha256` for source tarball is a placeholder until the first tag is published (see [docs/RELEASE.md](./docs/RELEASE.md))

[Unreleased]: https://github.com/ashlrai/locus/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/ashlrai/locus/releases/tag/v0.1.0
