# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Open-source packaging: CI/release workflows, CONTRIBUTING, SECURITY, CODE_OF_CONDUCT, adapter/MCP docs

## [0.1.0] — 2026-08-06

Initial public phase **0 + 1** cut of **Locus** — identity plane for coding agents.

### Added

#### Phase 0 — daemon-less control plane

- Workspace crates: `locus-core`, `locus-cli` (`locus`), `locus-mcp`
- Binding store under `~/.locus` (overridable via `LOCUS_HOME`)
- `locus init`, `pin`, `leave`, `whoami`, `status`, `binding list|show|add|rm`
- HMAC-sealed session pins (tampering fails closed)
- Workspace `.locus.toml` — default binding, allowlist, `require-pin`
- `locus exec` — scrub ambient identity env, resolve CredentialRefs into child only
- Private worker dirs for GH/AWS-style isolation under `~/.locus/workers/<session>/`
- Shell hooks: `locus hook zsh|bash|fish`
- Credential refs: `phm:NAME`, `env:VAR`, `test:VALUE` (test only with `LOCUS_ALLOW_TEST_CREDS=1`)

#### Phase 1 — MCP, adapters, setup

- `locus-mcp` stdio multiplexor (JSON-RPC 2.0 / MCP subset)
- Control tools: `locus_whoami`, `locus_status`, `locus_list_bindings`, `locus_request_pin`, `locus_providers` (when pinned)
- Agents **cannot** pin — request only
- Provider adapters (identity / scope tools + freeze): **Supabase**, **GitHub**, **Vercel**
- Scope freeze — model-supplied account selectors denied when binding freezes them
- Policy: allow/deny defaults + `require_approval` globs (destructive stubs need `confirm=true`)
- `locus setup --client claude|cursor|codex` for MCP config merge
- `locus doctor` readiness checks
- Isolation and adapter unit tests in `locus-core`

### Security

- Session seal verification on MCP tool list/call paths
- Unbound session ⇒ control tools only
- MCP never returns secret values
- Workspace allowlist + optional `--force` for out-of-allowlist pins

### Known limitations

- Adapters are synthetic (no live upstream Supabase/Vercel MCP fan-out yet)
- Dual-control, team binding graph, continuous whoami drift, AWS/CF/Resend — later phases (see [PLAN.md](./PLAN.md))

[Unreleased]: https://github.com/ashlrai/locus/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/ashlrai/locus/releases/tag/v0.1.0
