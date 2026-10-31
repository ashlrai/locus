# Minimum supported Rust version (MSRV)

The workspace declares the floor once, in `[workspace.package]` of the root
[Cargo.toml](../Cargo.toml):

```toml
rust-version = "1.89"
```

Every member crate inherits it with `rust-version.workspace = true`, so
`cargo metadata` reports the same `rust_version` on `locus-core`, `locus-cli`,
and `locus-mcp`. The npm wrappers' cargo-fallback guards and the README
source-install section name the same floor; the
`crates/locus-core/tests/rust_version.rs` suite fails if any of those
surfaces drift apart.

## Why it exists — issue #167

`cargo install --git` does not read the checkout's `rust-toolchain.toml`; it
uses the installer shell's default toolchain. On a toolchain older than the
locked dependencies' edition, the build died mid-way with a parse error —
on cargo 1.83.0, `feature 'edition2024' is required` while reading
`zeroize_derive` 1.5.0 — instead of a minimum-version message (measured in
[BENCHMARKS.md](./BENCHMARKS.md)). With `rust-version` declared, Cargo ≥1.85
parses the edition-2024 lockfile entries and refuses up front with
`requires rustc 1.89`, which names the fix (`rustup update`) instead of a
cryptic dependency error. Cargo ≤1.84 still fails on the edition-2024
manifests first, but now fails before compiling rather than mid-install.

## What sets the floor

`std::fs::File::lock` — the `file_lock` library feature, stabilized in
**Rust 1.89.0** ([rust-lang/rust#130994](https://github.com/rust-lang/rust/issues/130994)).
`locus-core` uses it for state-file mutual exclusion at five call sites:

- `crates/locus-core/src/budget.rs`
- `crates/locus-core/src/credential_audit.rs`
- `crates/locus-core/src/credential_migration.rs` (two sites)
- `crates/locus-core/src/store.rs`

No dependency in the committed `Cargo.lock` requires a newer compiler, so the
source-level floor above is the workspace MSRV.

## Measured

`cargo check --workspace --locked` against the committed `Cargo.lock`,
macOS arm64 (Apple Silicon), 2026-10-30:

| Toolchain | Command | Result |
|-----------|---------|--------|
| rustc/cargo 1.88.0 | `cargo check --workspace --locked` | Refused before compiling: `error: rustc 1.88.0 is not supported by the following packages: locus-cli@0.5.2 requires rustc 1.89 ...` — the declared floor reporting correctly. |
| rustc/cargo 1.88.0 | `cargo check --workspace --locked --ignore-rust-version` | All locked dependencies compile (the lockfile has ~260 crates); `locus-core` fails with 5 × `E0658: use of unstable library feature 'file_lock'` at the `File::lock` sites above. |
| rustc/cargo 1.89.0 | `cargo check --workspace --locked` | Finished in 2m31s, no warnings. |
| rustc/cargo 1.89.0 | `cargo install --git file://<checkout> locus-cli locus-mcp --locked` | `Successfully installed locus-cli, locus-mcp` (release build of locus-cli in 13m11s, locus-mcp in 16m56s on a loaded machine); both binaries run `--version`. The command shape from issue #167 works on the declared floor. |

Cross-check from the original investigation (Linux x86_64, 2026-10-26, see
[BENCHMARKS.md](./BENCHMARKS.md)):

| Toolchain | Command | Result |
|-----------|---------|--------|
| rustc/cargo 1.83.0 | `cargo install --git … locus-cli locus-mcp --locked` | Exit 101 in 1.9s: `feature 'edition2024' is required` while parsing `zeroize_derive` 1.5.0. This is the issue-#167 report; predates the `rust-version` declaration. |
| rustc/cargo 1.99.1 | same, `RUSTUP_TOOLCHAIN=stable` | Exit 0 in 86.5s. |

## Guardrails

- CI job `msrv` (`.github/workflows/ci.yml`) runs
  `cargo +1.89.0 check --workspace --locked` on pushes and on pull requests
  that touch `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `.cargo/`, or
  `**/*.rs`.
- `cargo test -p locus-core --test rust_version` asserts the workspace
  declares a semver floor, every member inherits or pins it, `cargo
  metadata --no-deps --locked` reports it on every package, and README / npm
  / Homebrew surfaces name the same version.

## Raising or lowering the floor

The declared value tracks the committed lockfile plus first-party source
requirements. To re-measure:

```bash
rustup toolchain install <candidate> --profile minimal
cargo +<candidate> check --workspace --locked --ignore-rust-version
```

A toolchain below the true floor fails inside a first-party crate (as 1.88.0
does on `file_lock`); a dependency-only failure points at lockfile drift, not
source requirements. Update `[workspace.package].rust-version`, the README
floor, `MIN_RUST_VERSION` in `npm/bin/locus.js` and
`npm-mcp/bin/locus-mcp.js`, and the CI `msrv` job's toolchain together — the
`rust_version` test enforces that they match.
