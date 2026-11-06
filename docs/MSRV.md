# Minimum supported Rust version

Locus requires **Rust 1.89 or newer** and a matching Cargo toolchain for
source builds. The workspace declares this minimum in `Cargo.toml`; each
member inherits it.

The minimum is verified against the committed lockfile on the current host by
checking the whole workspace, including test and example targets:

```bash
rustup toolchain install 1.89.0 --profile minimal
cargo +1.89.0 check --workspace --all-targets --locked
```

This check covers the host platform and locked dependencies, not every target
platform. Rust 1.83 cannot parse the locked `idna_adapter` dependency's 2024
edition; Cargo 1.85 through 1.88 can parse it but reject the declared minimum.

If your compiler is older, install and select a supported toolchain with rustup:

```bash
rustup update stable
rustup default stable
rustc --version
cargo --version
```

Alternatively, use `cargo +1.89.0` after installing that toolchain. A checkout's
`rust-toolchain.toml` can select a toolchain independently of your default.
