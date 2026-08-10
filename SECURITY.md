# Security Policy

Locus is an **identity plane** for coding agents: it pins sessions to a Binding and makes wrong-account action mechanically hard. This document summarizes the threat model and how to report issues.

Full design detail lives in **[DESIGN.md §9](./DESIGN.md)** (assets, adversaries, non-goals, invariants).

## Supported versions

| Version | Supported |
|---------|-----------|
| `0.1.x` (main) | Yes — active development |
| pre-0.1 tags / unreleased spikes | Best effort |

## Reporting a vulnerability

**Please do not open a public GitHub issue for security bugs.**

1. Email **security@ashlr.ai** (preferred), or open a **private** security advisory on the [ashlrai/locus](https://github.com/ashlrai/locus) repository if you have access.
2. Include:
   - Affected commit / version / platform
   - Reproduction steps (minimal, if possible)
   - Impact (e.g. cross-binding credential leak, seal forgery, scope freeze bypass)
   - Whether you plan a public write-up and preferred timeline
3. We will acknowledge receipt as soon as practical and coordinate fix + disclosure.

We appreciate coordinated disclosure. Credit is given unless you ask otherwise.

## Threat model (summary)

### What we protect

| Asset | Intent |
|-------|--------|
| **Provider credentials** | Resolved only into worker/child env; never returned by MCP tools |
| **Tenant / account selection** | Session sealed to one Binding; tools catalog is exclusive |
| **Scope freezes** | `project_ref`, `team_id`, org/repo allowlists cannot be overridden by the model |
| **Session integrity** | V3 HMAC seals plus a live broker subject/generation bind the exact record, backing, expiry, and authority |
| **Workspace boundaries** | `.locus.toml` `allowed_bindings` blocks wrong-tenant pins (unless audited `--force`) |

### High-priority threats we design against

| Threat | Mitigation (design) |
|--------|---------------------|
| Confused deputy (agent on A affects B) | Separate pin/catalog; no cross-binding tools |
| Prompt injection → re-pin | Agents cannot pin; `locus_request_pin` only |
| Arg smuggling (`project_ref` swap) | Adapter scope freeze |
| Ambient CLI race (`gh auth switch`) | Private `GH_CONFIG_DIR` / scrubbed env in `locus exec` |
| Ambient credential inheritance | Scrub known identity env vars; inject only resolved refs for the pin |
| Seal forgery / replay | HMAC covers session fields; an in-memory supervised broker independently binds their digest to a monotonic generation and rejects stale records |
| Approval id path traversal | Ids constrained to safe charset; joined path must stay under `approvals/` |
| Confirm / approval_id injection into digests | `args_digest` strips control + secret-like keys before hashing |

### Approval authority and dual-control

Destructive tools remain blocked until Locus receives independently authenticated
external authorization. Local CLI/dashboard principal strings are advisory
review labels only and never become execution authority.

| Policy field | Effect |
|--------------|--------|
| `require_approval = ["*.delete*", …]` | Tool blocked pending one valid external authorization envelope |
| `dual_control = ["vercel.deploy.prod", …]` | Matching tools require two externally authenticated approvers |
| `dual_control_all_approvals = true` | Every `require_approval` match requires two external approvers |

Flow:

1. Agent hits a gated tool → Locus creates `$LOCUS_HOME/approvals/appr_….json` with `status=pending` and an `args_digest` (raw args **never** stored).
2. Operators may record local review evidence with `locus approve grant appr_… --as alice`; the record remains pending.
3. A second local label is still advisory and cannot satisfy dual-control.
4. Only a closed external envelope, issued through a non-agent-accessible capability and verified against an independent trust root, may authorize the exact request. No such verifier ships in this release, so provider execution remains blocked and agents must not retry after local labels.

**Secrets never appear** in approval files, audit JSONL, or MCP tool results — only digests, ids, tool names, and principal labels. Principal names are restricted to `[A-Za-z0-9_-]` (no path separators).

**Rate limiting:** Locus does not yet enforce request rate limits on advisory-label or pending-record creation in process. Operators should treat approval files as sensitive control-plane state. External envelope replay protection, expiry, identity binding, and OS attestation are required before authoritative approvals can be enabled.

### Explicit non-goals (out of scope)

- **Root / malware on the developer machine** — if the OS is fully compromised, all local tools are in scope for the attacker.
- **Same-UID code that can inspect or modify the trusted operator/broker process memory** — filesystem mode `0600` does not isolate `daemon.key` from code already running as the same user. The broker prevents possession of that file key alone from forging current authority, but it cannot survive theft of the in-memory control capability through an OS debugging/process-memory boundary.
- **A human who deliberately pins the wrong client** (`locus pin personal --force`) — Locus does not second-guess intentional human pin switches beyond workspace allowlists and audit hooks.
- **Replacing cloud IAM / SSO** — Locus complements provider IAM; it does not replace org SSO policies.
- **Malicious code inside a worker that already holds that binding’s credentials** — blast radius is intentionally one binding. Optional OS sandbox (`LOCUS_WORKER_SANDBOX=1`) is additive best-effort (macOS Seatbelt; Linux bubblewrap or path-only fallback tagged `path` — not a VM). See [docs/workers.md](./docs/workers.md).
- **Phase 1 synthetic adapters** — current Supabase/GitHub/Vercel tools are identity/scope stubs and policy demos; they do **not** yet fan out to real upstream MCP APIs. Remote-call isolation for live workers is roadmap (see PLAN / DESIGN).

### Invariants we care about (testable)

From DESIGN.md:

- `tools/call` succeeds only with a valid seal and tool provider ∈ binding  
- Worker/exec env for binding A must not contain credential material for B  
- Unbound session ⇒ tool catalog is only control tools (`locus_*`)  
- Agent-initiated pin must not change state without human action  

See also the Security model section in [README.md](./README.md).

## Safe disclosure of non-security bugs

Use public GitHub issues for:

- Crash bugs that do not leak secrets across bindings
- DX / docs / adapter feature requests
- CI or packaging problems

When in doubt whether an issue is security-sensitive, use the private channel above.

## Hardening tips for operators

- Store secrets as explicit **CredentialRefs** (`phm:NAME` or `env:VAR`), never bare names or raw tokens in binding TOML. Production binaries reject `test:` regardless of environment.
- Prefer workspace `require_pin = true` and tight `allowed_bindings` in client repos.
- Treat malformed `.locus.toml` as an `UNSAFE` doctor finding; pin and autopin refuse to continue, including with `--force`.
- Use `locus doctor` and `locus whoami` before destructive agent work.
- Keep Phantom (or your vault) and Locus updated together when using `phm:` refs.
- For production-like deploys, set `dual_control` globs (or `dual_control_all_approvals = true`) to declare the required external authority threshold. Local laptop labels do not satisfy it.
- Keep `$LOCUS_HOME` mode-restricted to block other OS users and treat `approvals/` and `audit/` as sensitive. Do not treat `daemon.key` mode `0600` as protection from same-UID code; delegated mutation/provider execution also requires a live broker capability.
- Review `locus approve list` regularly; `--ttl` is reserved for future externally authenticated grants and does not make local labels authoritative.
