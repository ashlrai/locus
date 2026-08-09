# Locus — Development Guide

**Prefer `locus enter` (or `locus pin`) before tool use** — identity is resolved at the gate, not in the prompt.

## Quick reference

```bash
export PATH="$HOME/.cargo/bin:$PATH"

cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all

cargo run -p locus-cli -- --help
cargo run -p locus-mcp --   # stdio MCP; do not print logs on stdout
```

Release binaries: `./target/release/locus`, `./target/release/locus-mcp`.

Smoke without touching `~/.locus`:

```bash
export LOCUS_HOME=/tmp/locus-dev
cargo run -p locus-cli -- quickstart   # or: init --with-samples && enter personal
cargo run -p locus-cli -- whoami
cargo run -p locus-cli -- doctor
```

## Architecture

3-crate Rust workspace:

| Crate | Role |
|-------|------|
| **locus-core** | Binding, Session, Seal (HMAC), Policy, Store (`~/.locus`), isolation env, CredentialRef resolve, adapters, workers |
| **locus-cli** | Human control plane: `init`, `pin`, `leave`, `whoami`, `exec`, `binding`, `workspace`, `setup`, `doctor`, `hook` |
| **locus-mcp** | Stdio MCP multiplexor — tools = control + **pinned binding only** |

Planes (see [docs/architecture.md](./docs/architecture.md)):

```
Clients → data plane (locus-mcp / exec)
            ├─ control: bindings + sealed sessions
            ├─ policy: allow / deny / require_approval
            ├─ credential: phm: / env: → worker env only
            └─ workers: synthetic adapters or MCP stdio children
```

**Sharp rule:** identity is resolved at the gate, not in the prompt. Agents cannot pin; they may `locus_request_pin` only.

## Conventions

- Fail closed: bad seal, scope mismatch, unknown provider path → deny/error
- CredentialRefs in bindings — never raw secrets in TOML or git
- Scope freeze on every account selector (`freeze_string_arg`)
- MCP stdout is protocol-only (no logging)
- `LOCUS_HOME` for tests; `LOCUS_ALLOW_TEST_CREDS=1` only for `test:` refs in unit tests
- fmt + clippy `-D warnings` must pass (CI)
- Adapters live in `crates/locus-core/src/adapters/`; guide: [docs/adapters.md](./docs/adapters.md)

## Key paths

| Path | Why |
|------|-----|
| `crates/locus-core/src/lib.rs` | Public surface + invariants |
| `crates/locus-core/src/isolation.rs` | Scrub ambient identity; build child env |
| `crates/locus-core/src/seal.rs` | Session HMAC |
| `crates/locus-core/src/adapters/mod.rs` | `ProviderAdapter`, freeze helpers, dispatch |
| `crates/locus-core/src/workspace.rs` | `.locus.toml` walk + allowlist |
| `crates/locus-mcp/src/main.rs` | MCP tool catalog + call gate |
| `DESIGN.md` | Full architecture + threat model |
| `AGENTS.md` | Agent-oriented rules and never-commit secrets |

## Docs

| Doc | Topic |
|-----|--------|
| [AGENTS.md](./AGENTS.md) | Full agent instructions |
| [docs/architecture.md](./docs/architecture.md) | System diagram |
| [docs/policy.md](./docs/policy.md) | Policy rules + approval CLI |
| [docs/firm-mode.md](./docs/firm-mode.md) | Multi-client / dual-control ops |
| [docs/agency-starter.md](./docs/agency-starter.md) | Agency starter kit + doctor single pane |
| [docs/mcp.md](./docs/mcp.md) | Client wiring |
| [docs/workers.md](./docs/workers.md) | Worker backends |
| [CONTRIBUTING.md](./CONTRIBUTING.md) | PR + adapter checklist |
| [SECURITY.md](./SECURITY.md) | Reporting |

## Sibling product

[Phantom](https://phm.dev) — secrets never enter the model. Compose via `credential_ref = "phm:NAME"`.
