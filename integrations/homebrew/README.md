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

Until the first tag + formula publish, install from source:

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo install --git https://github.com/ashlrai/locus --package locus-cli --locked
cargo install --git https://github.com/ashlrai/locus --package locus-mcp --locked
```

Or test the local formula (requires a matching tag / valid `sha256`, or use `--HEAD`):

```bash
# HEAD (no tag/sha needed)
brew install --HEAD --formula ./integrations/homebrew/Formula/locus.rb

# After tagging v0.1.0 and filling in sha256:
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

1. Confirm [`.github/workflows/release.yml`](../../.github/workflows/release.yml) uploaded `locus-<target>.tar.gz` assets.
2. **Source formula:** update `version` and `sha256` for  
   `https://github.com/ashlrai/locus/archive/refs/tags/vX.Y.Z.tar.gz`:

   ```bash
   curl -sL "https://github.com/ashlrai/locus/archive/refs/tags/vX.Y.Z.tar.gz" | shasum -a 256
   ```

3. **Prebuilt formula (optional):** compute per-target sha256 of the release assets and uncomment/switch the prebuilt block in `Formula/locus.rb`.
4. PR the same bump into the live tap (`homebrew-ashlr` or `homebrew-locus`).
5. Bump `VERSION` in `npm/bin/locus.js` and `npm-mcp/bin/locus-mcp.js` if publishing npm wrappers.

## Formula behavior

| Mode | What it does |
|------|----------------|
| **Source (default)** | `depends_on "rust" => :build`; `cargo install` of `locus-cli` + `locus-mcp` |
| **HEAD** | `brew install --HEAD` builds from `main` |
| **Prebuilt (optional)** | Downloads release tarballs; installs both binaries |

Both `locus` and `locus-mcp` are installed into `$(brew --prefix)/bin`.
