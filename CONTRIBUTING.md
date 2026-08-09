# Contributing to Locus

Thanks for helping make wrong-account action mechanically impossible.

## Prerequisites

- Rust **stable** (see [`rust-toolchain.toml`](./rust-toolchain.toml))
- Cargo on `PATH` — typically:

  ```bash
  export PATH="$HOME/.cargo/bin:$PATH"
  ```

- Optional: [Phantom](https://phm.dev) CLI (`phantom`) if you want to exercise `phm:` credential refs end-to-end

## Clone and build

```bash
git clone https://github.com/ashlrai/locus.git
cd locus
export PATH="$HOME/.cargo/bin:$PATH"

cargo build --workspace
cargo build --release -p locus-cli -p locus-mcp
```

Binaries land at:

- `./target/release/locus`
- `./target/release/locus-mcp`

Install locally:

```bash
cargo install --path crates/locus-cli
cargo install --path crates/locus-mcp
```

## Test

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo test --workspace
```

Isolation and adapter behavior live primarily in `locus-core` unit tests. Prefer `LOCUS_HOME` for any local smoke tests so you do not touch `~/.locus`:

```bash
export LOCUS_HOME=/tmp/locus-dev
locus init --with-samples
locus pin personal
locus whoami
```

## Lint and format

CI enforces these on every PR:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

Auto-fix format:

```bash
cargo fmt --all
```

## Project layout

| Path | Role |
|------|------|
| `crates/locus-core` | Bindings, sessions, seal, policy, store, adapters |
| `crates/locus-cli` | `locus` binary — pin, exec, binding, setup |
| `crates/locus-mcp` | Stdio MCP multiplexor hard-scoped to the active pin |
| `DESIGN.md` | Architecture, threat model, adapter model |
| `PLAN.md` | Phased roadmap |
| `docs/` | Contributor-facing how-tos |

## Adding a provider adapter

Adapters are the **only** place provider-specific knowledge should live. Phase 1 adapters expose **safe identity / scope tools** and enforce **scope freeze** (model cannot override frozen `project_ref`, `team_id`, orgs, etc.). Full upstream MCP workers come later.

See **[docs/adapter-sdk.md](./docs/adapter-sdk.md)** (preferred) and **[docs/adapters.md](./docs/adapters.md)**. Template: `examples/adapters/_template/`. Catalog: `adapters/manifest.toml`.

Short checklist:

1. Add `crates/locus-core/src/adapters/<provider>.rs` implementing `ProviderAdapter` (or start from the template skeleton).
2. Register in `adapter_for()` in `adapters/mod.rs`.
3. Use `freeze_string_arg` (or equivalent) for every account selector the model might smuggle.
4. Never return secret values from tool results.
5. Mark destructive tools with `destructive: true` and rely on policy `require_approval`.
6. Add unit tests for freeze deny + happy path identity tools.
7. Document hard scope knobs in `docs/adapters.md` / adapter-sdk and update `adapters/manifest.toml`.

## Coding guidelines

- **Fail closed** — invalid seals, unknown providers without a safe path, scope mismatches → deny/error, not soft allow.
- **No ambient identity** — do not read global `gh auth` / default AWS profile as the source of truth for a pin.
- **CredentialRefs only** — bindings store `phm:NAME` / `env:VAR`, never raw tokens; production binaries reject `test:`.
- **MCP stdout is sacred** — `locus-mcp` must not print logs to stdout (protocol is newline-delimited JSON-RPC on stdio).
- Prefer small, tested modules over large control-flow in `main`.

## Pull requests

1. Branch from `main`.
2. Keep PRs focused (one adapter, one CLI surface, one isolation fix).
3. Ensure `fmt`, `clippy -D warnings`, and `cargo test` pass.
4. Describe **what isolation property** changes (or does not).
5. Link related `DESIGN.md` sections when changing security-sensitive code.

## Reporting security issues

Do **not** open a public issue for vulnerabilities. See [SECURITY.md](./SECURITY.md).

## License

By contributing, you agree that your contributions will be licensed under the MIT License (see [LICENSE](./LICENSE)).
