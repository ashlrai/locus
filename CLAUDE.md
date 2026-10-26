# Locus: Development Guide

**Prefer `locus enter` (or `locus pin`) before tool use**: identity is resolved at the gate, not in the prompt.

## Quick reference

```bash
export PATH="$HOME/.cargo/bin:$PATH"

cargo build --workspace
cargo test --workspace
cargo test -p locus-cli --test doc_facts # CLI/MCP catalog and example freshness
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all

cargo run -p locus-cli -- --help
cargo run -p locus-mcp --   # stdio MCP; do not print logs on stdout
```

A client-launched `locus-mcp` serves control tools only. Provider tools for the
pinned binding need `locus mcp --supervised` (see [docs/mcp.md](./docs/mcp.md)).

Release binaries: `./target/release/locus`, `./target/release/locus-mcp`.

Smoke without touching `~/.locus`. Run it as one shell script (one Bash call):
the pin's authority broker is supervised by the process that pinned, so it
dies with that shell, and with `cargo run` it dies as soon as cargo exits.

```bash
cargo build --workspace
export LOCUS_HOME="$(mktemp -d)" PATH="$PWD/target/debug:$PATH"
locus quickstart                 # or: init --with-samples, then the hook line, then enter personal
eval "$(locus hook bash)"        # exports LOCUS_CONTROL_CAPABILITY for whoami/pin/leave
locus enter personal            # establish this shell's broker-backed pin
locus whoami
locus doctor                     # setup advisories exit 0; actionable WARN 1; UNSAFE 2
```

A later shell sees `no current broker endpoint ... human re-pin required`.
In the new shell, restore the same scratch `LOCUS_HOME`, add `target/debug` to `PATH`, load
`eval "$(locus hook bash)"`, then run `locus enter personal`.
Only if enter is refused (expired pin, bad seal), run `locus leave --force` first.

## Architecture

3-crate Rust workspace:

| Crate | Role |
|-------|------|
| **locus-core** | Binding, Session, Seal (HMAC), Policy, Store (`~/.locus`), isolation env, CredentialRef resolve, adapters, workers |
| **locus-cli** | Human control plane. Daily use: `enter`, `switch`, `pin`, `leave`, `renew`, `whoami`, `status`, `exec`, `run`, `binding`. Setup: `init`, `quickstart`, `onboard`, `setup`, `agent`, `doctor`, `hook`, `mcp`, `workspace`, and more. `locus --help` is the full grouped list |
| **locus-mcp** | Stdio MCP multiplexor: tools = control + **pinned binding only** |

Planes (see [docs/architecture.md](./docs/architecture.md)):

```
Clients → data plane (locus-mcp / exec)
            ├─ control: bindings + sealed sessions
            ├─ policy: allow / deny / require_approval
            ├─ credential: env: → worker env only; phm: scoped bridge unavailable
            └─ workers: synthetic adapters or MCP stdio children
```

**Sharp rule:** identity is resolved at the gate, not in the prompt. Agents cannot pin; they may `locus_request_pin` only.

## Conventions

- Fail closed: bad seal, scope mismatch, unknown provider path → deny/error
- CredentialRefs in bindings: never raw secrets in TOML or git
- Scope freeze on every account selector (`freeze_string_arg`)
- MCP stdout is protocol-only (no logging)
- `LOCUS_HOME` for tests; production binaries reject `test:` refs regardless of environment
- fmt + clippy `-D warnings` must pass (CI)
- CLI exits only through `exit::ExitCode`; `--json` check/status output goes through `json_out` (schema id first); a new command must pick a `DryRunSupport` class (docs/agent-contract.md)
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
| [docs/mcp.md](./docs/mcp.md) | Client wiring (Claude Code, Cursor, Codex, Gemini CLI, Grok Build) |
| [docs/agent-contract.md](./docs/agent-contract.md) | `--json` schema ids, exit codes, `--dry-run`, non-interactive rules |
| [docs/onboarding.md](./docs/onboarding.md) | `locus onboard` + clients × tenants walkthrough |
| [docs/workers.md](./docs/workers.md) | Worker backends |
| [CONTRIBUTING.md](./CONTRIBUTING.md) | PR + adapter checklist |
| [SECURITY.md](./SECURITY.md) | Reporting |

## Sibling product

[Phantom Secrets](https://github.com/ashlrai/phantom-secrets) protects credentials. Locus retains `phm:NAME` references but cannot resolve them until a supported scoped bridge exists. Use explicit `env:VAR` references supplied outside agent context; never capture trusted-terminal reveal output. This source change is unreleased; see [credential compatibility](./docs/credential-compatibility.md).
