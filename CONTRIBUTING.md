# Contributing to Locus

Thanks for helping make wrong-account action mechanically impossible.

## Prerequisites

- Rust **stable** (see [`rust-toolchain.toml`](./rust-toolchain.toml))
- Cargo on `PATH` — typically:

  ```bash
  export PATH="$HOME/.cargo/bin:$PATH"
  ```

- Credential resolution uses explicit `env:VAR` references with synthetic values in tests. Stored `phm:` references fail closed until a supported scoped bridge exists; do not capture Phantom reveal output. See [credential compatibility](./docs/credential-compatibility.md).

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

### Investigating repeated compilation during install

The [first-run investigation](./docs/BENCHMARKS.md) for [issue #168](https://github.com/ashlrai/locus/issues/168) recorded `locus-core` compiling again while installing the two binaries with one `cargo install --git` command. That observation alone does not establish whether the compiler invocations were identical or whether a change would improve the build.

To investigate the committed revision of a local checkout, run this from its root. It uses a fresh build directory and install root, so it neither relies on an existing target cache nor installs binaries into your usual Cargo bin directory:

```bash
investigation_dir=$(mktemp -d)
cargo install --git "file://$(pwd)" locus-cli locus-mcp --locked \
  --root "$investigation_dir/install" --target-dir "$investigation_dir/target" \
  --message-format=json > "$investigation_dir/artifacts.jsonl" 2> "$investigation_dir/cargo.log"
```

This installs from the checkout's committed `HEAD`, not uncommitted changes. To investigate the original remote install, replace the `--git` URL with `https://github.com/ashlrai/locus` and add `--rev <commit>` to fix the revision. Record the commit, Cargo and Rust versions, platform, and command when comparing runs. The first-run report used commit `e969d9c4`; dependency resolution and results can change across revisions and toolchains.

Inspect `cargo.log` for the order of `Compiling locus-core` and `Finished` lines. The JSON stream gives more detail for each core artifact:

```bash
python3 - "$investigation_dir/artifacts.jsonl" <<'PY'
import json, sys
with open(sys.argv[1], encoding="utf-8") as stream:
    for line in stream:
        message = json.loads(line)
        if message.get("reason") == "compiler-artifact" and message["target"]["name"] == "locus_core":
            print({key: message[key] for key in ("package_id", "target", "profile", "features", "filenames", "fresh")})
PY
```

Compare the package identity, target kind, profile, features and artifact paths; `fresh: true` means Cargo reused an artifact, while `fresh: false` indicates it was built in that invocation. Separate install steps, feature sets, profiles, host/target builds, or dependency versions can produce different compilation units. A repeated name alone does not prove duplicate work. For deeper investigation `-vv` can show rustc commands, but its output can contain environment values and local paths; review and redact it before sharing.

On macOS with commit `7233ae79ceff`, Cargo 1.95.0 and rustc 1.95.0, the command above produced two `locus_core` artifact records, both `fresh: false`. Their package identity, target, profile and features matched, but their artifact paths differed. Those fields do not explain the differing artifact identities; compare the compiler invocations before attributing a cause.

## Test

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo test --workspace
```

### Minimum supported Rust version

The workspace declares Rust 1.89 as its minimum supported Rust version (MSRV).
When changing Rust code or dependencies, verify the locked workspace, including
test and example targets, with that toolchain:

```bash
rustup toolchain install 1.89.0 --profile minimal
cargo +1.89.0 check --workspace --all-targets --locked
```

This checks the current host and lockfile; repeat on supported target platforms
when changing platform-specific code or dependencies. If the minimum must rise,
update the workspace declaration, installation documentation and npm fallback
messages together. The Rust-version tests check those surfaces for agreement.

Isolation and adapter behavior live primarily in `locus-core` unit tests. Prefer `LOCUS_HOME` for any local smoke tests so you do not touch `~/.locus`:

```bash
export LOCUS_HOME=/tmp/locus-dev
locus init --with-samples
locus pin personal
locus whoami
```

Docs checks (Node 18+, no dependencies):

```bash
node scripts/check-readme-links.mjs   # every README link / anchor / image resolves
node --test scripts/check-readme-links.test.mjs
node scripts/freshness/check.mjs      # generated release blocks are in sync
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
6. Conflict in a high-churn path? `CHANGELOG.md` unions itself via `.gitattributes`; `node scripts/resolve-merge-conflicts.mjs --continue` deterministically resolves `CLAUDE.md`/`README.md` and regenerates `Cargo.lock`/`bun.lock`/`package-lock.json` — never hand-merge lockfiles. See [docs/conflict-drivers.md](./docs/conflict-drivers.md).

## Reporting security issues

Do **not** open a public issue for vulnerabilities. See [SECURITY.md](./SECURITY.md).

## License

By contributing, you agree that your contributions will be licensed under the MIT License (see [LICENSE](./LICENSE)).
