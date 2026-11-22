# Homebrew packaging for Locus

This directory mirrors the Homebrew formula for review alongside the code.
The tap repo is the delivery channel end users install from.

## End-user install (when published)

```bash
# Preferred once the formula is in the Ashlr tap:
brew install ashlrai/tap/locus
# or, if shipped under homebrew-ashlr:
# brew install ashlrai/ashlr/locus

# Dedicated tap (if you create ashlrai/homebrew-locus):
brew tap ashlrai/locus
brew install locus
```

To install from current Git main instead of the tap (requires Rust 1.89 or
newer), use the current source's `locus-cli` package, which installs both binaries:

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo install --git https://github.com/ashlrai/locus locus-cli --locked
```

The mirrored formula below targets an older source release with separate binary
packages. Its two-install recipe may collide on `--HEAD`, because current main
bundles both binaries in `locus-cli`. Test that path separately before use.
For the pinned formula, a matching tag and valid `sha256` are required:

```bash
# Test the mirrored v0.5.0 formula:
brew install --formula ./integrations/homebrew/Formula/locus.rb
locus --version
brew uninstall locus
```

## Where to publish the formula

### Option A — add to `ashlrai/homebrew-ashlr` (recommended)

1. Copy `Formula/locus.rb` into the [homebrew-ashlr](https://github.com/ashlrai/homebrew-ashlr) repo under `Formula/`.
2. Fill in a real `sha256` for the tagged source tarball (source-build formula), **or** switch to the prebuilt URL block in the formula comments once `release.yml` assets exist.
3. Push; users install with:

   ```bash
   brew install ashlrai/ashlr/locus
   ```

4. Mention Locus in that tap’s README install list.

### Option B — dedicated tap `ashlrai/homebrew-locus`

1. Create a public repo `ashlrai/homebrew-locus` with layout:

   ```
   Formula/
     locus.rb
   README.md
   ```

2. Copy this formula in, set `url` + `sha256` for the release.
3. Users:

   ```bash
   brew tap ashlrai/locus
   brew install locus
   ```

   Homebrew maps `ashlrai/locus` → `github.com/ashlrai/homebrew-locus`.

## Release checklist

On every tagged release (`vX.Y.Z`):

1. Confirm [`.github/workflows/release.yml`](../../.github/workflows/release.yml) uploaded the `locus-<target>.tar.gz` assets and their `.sha256` sidecars
   (targets: `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu`).
2. **Source formula:** update `version` and `sha256` for  
   `https://github.com/ashlrai/locus/archive/refs/tags/vX.Y.Z.tar.gz`:

   ```bash
   curl -sL "https://github.com/ashlrai/locus/archive/refs/tags/vX.Y.Z.tar.gz" | shasum -a 256
   ```

3. **Prebuilt formula (optional):** take each target's sha256 from its `.sha256` sidecar (or recompute it from the asset) and uncomment/switch the prebuilt block in `Formula/locus.rb`.
4. PR the same bump into the live tap (`homebrew-ashlr` or `homebrew-locus`).

npm wrapper versions are not a Homebrew step. They are synchronized before the
tag with `node scripts/release-version.mjs sync X.Y.Z`, CI and `release.yml`
refuse a mismatch, and `release.yml` publishes both npm packages. See
[docs/RELEASE.md](../../docs/RELEASE.md).

## Formula behavior

This describes the mirrored v0.5.0 formula. Current main builds both binaries
from `locus-cli`, so its separate `locus-mcp` install step needs updating
before the formula targets a newer source release.

| Mode | What it does |
|------|----------------|
| **Source (default)** | `depends_on "rust" => :build`; `cargo install` of `locus-cli` + `locus-mcp` |
| **HEAD** | `brew install --HEAD` builds from `main` |
| **Prebuilt (optional)** | Downloads release tarballs; installs both binaries |

Both `locus` and `locus-mcp` are installed into `$(brew --prefix)/bin`.
