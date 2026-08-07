# Releasing Locus

Version lives in the workspace root [`Cargo.toml`](../Cargo.toml) under
`[workspace.package] version` (currently **0.1.0**). Crates inherit via
`version.workspace = true`. npm packages under `npm/` and `npm-mcp/` keep a
matching `"version"`.

## Preconditions

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

- [ ] `CHANGELOG.md` has a dated `## [X.Y.Z]` section with the full feature list
- [ ] Working tree clean (or intentional release commit)
- [ ] CI green on `main`

## Tag and push

Tags must match `v*` so [`.github/workflows/release.yml`](../.github/workflows/release.yml) runs.

```bash
# From a clean main at the release commit:
git tag -a v0.1.0 -m "Locus v0.1.0"
git push origin v0.1.0
# or push all tags:
git push --tags
```

Annotated tags are preferred (`-a`). Lightweight tags also trigger the workflow.

### First release (v0.1.0)

```bash
git tag -a v0.1.0 -m "Locus v0.1.0 — initial public release"
git push origin main
git push origin v0.1.0
```

## What release.yml publishes

On `push` of tag `v*`, the workflow:

1. Builds release binaries for:
   - `aarch64-apple-darwin`
   - `x86_64-apple-darwin`
   - `x86_64-unknown-linux-gnu`
2. Packs each as `locus-<target>.tar.gz` containing:
   - `locus-<target>/locus`
   - `locus-<target>/locus-mcp`
   - `README.md`, `LICENSE` (best-effort)
3. Creates a GitHub Release with `generate_release_notes: true` and attaches
   those tarballs as assets.

Asset URLs look like:

```text
https://github.com/ashlrai/locus/releases/download/v0.1.0/locus-aarch64-apple-darwin.tar.gz
https://github.com/ashlrai/locus/releases/download/v0.1.0/locus-x86_64-apple-darwin.tar.gz
https://github.com/ashlrai/locus/releases/download/v0.1.0/locus-x86_64-unknown-linux-gnu.tar.gz
```

Source archive (GitHub auto):

```text
https://github.com/ashlrai/locus/archive/refs/tags/v0.1.0.tar.gz
```

## After the tag (Homebrew / npm)

### Homebrew (source formula)

1. Compute source tarball sha256:

   ```bash
   curl -sL "https://github.com/ashlrai/locus/archive/refs/tags/v0.1.0.tar.gz" | shasum -a 256
   ```

2. Update `integrations/homebrew/Formula/locus.rb`: set `version` and replace the
   placeholder `sha256`.
3. Copy the formula into the live tap (`ashlrai/homebrew-ashlr` or
   `ashlrai/homebrew-locus`) and open a PR. See
   [integrations/homebrew/README.md](../integrations/homebrew/README.md).

### Homebrew (optional prebuilt)

Once release assets exist, switch the formula body to the commented prebuilt
block in `locus.rb` and fill per-target sha256 digests:

```bash
curl -sL "https://github.com/ashlrai/locus/releases/download/v0.1.0/locus-aarch64-apple-darwin.tar.gz" | shasum -a 256
# …repeat for x86_64-apple-darwin and x86_64-unknown-linux-gnu
```

### npm wrappers

Bump `VERSION` / `package.json` version in `npm/` and `npm-mcp/` if needed, then
publish from those packages (they download the matching release binary).

## Verify a release

```bash
# GitHub CLI
gh release view v0.1.0
gh release download v0.1.0 -p 'locus-*.tar.gz' -D /tmp/locus-rel

# Local binary
tar -xzf /tmp/locus-rel/locus-aarch64-apple-darwin.tar.gz -C /tmp
/tmp/locus-aarch64-apple-darwin/locus --version
```

## Hotfix / re-tag policy

Do **not** move an existing published tag. Bump patch (`v0.1.1`), document in
CHANGELOG, tag, and push.
