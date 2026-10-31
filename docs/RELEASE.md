# Releasing Locus

Version lives in the workspace root [`Cargo.toml`](../Cargo.toml) under
`[workspace.package] version` (currently **0.6.1**, prepared for release). Crates inherit via
`version.workspace = true`. npm packages under `npm/`, `npm-mcp/`, and
`apps/web/` keep a matching `"version"` (wrappers also set `VERSION` in
`npm/bin/locus.js` and `npm-mcp/bin/locus-mcp.js`).

## 0.6.1 release candidate — prepared 2026-10-31

This checkout prepares 0.6.1; a version bump is not publication evidence.
Once the final candidate commit is reviewed and frozen, run `ci.yml` with
`workflow_dispatch` on its ref and verify success on that exact commit SHA
before creating its non-forced tag. The candidate may be a release branch based
on a published tag; do not imply it was merged to main unless it actually was.
The release workflow requires that evidence. Current manual CI runs Linux
stable full checks and Linux rustc 1.89 checking, not native macOS/Windows tests.
The tag workflow builds Linux x64 and macOS arm64/x64 archives, checks their
SHA256 sidecars, exports an unsigned adapter manifest, creates the GitHub
Release, then publishes both npm packages through trusted OIDC. Verify every
stage and both registry versions; a GitHub Release alone does not prove npm
publication. Only then update `scripts/freshness/published.json` to the verified
version, run `node scripts/freshness/sync.mjs` and
`node apps/web/discoverability.mjs`, and publish that reviewed docs
update. Until then public install pins and release badges retain the previous
verified release; source manifests describe the candidate. Do not provision credentials or change protections to bypass gates.

Run the focused TypeScript drop-in tests with Node 22.6+ separately from the
Node 20 full-suite job, and record whether `scripts/e2e.sh` below was run rather
than inferring its result from the manual CI job. After updating canonical
versions and the changelog, regenerate and check both `scripts/freshness` and
`apps/web/discoverability.mjs`. Root `server.json` must match the source candidate;
the website `apps/web/public/server.json` separately describes the verified
published package.
Homebrew's live tap, the live website and MCP registry submission are separate
channels; verify them independently and never invent checksums before assets exist.

## Preconditions

The Linux full `ci.yml` job invokes `scripts/e2e.sh` after the release build
on scheduled and manual runs. The shell suite uses isolated Locus homes and
synthetic fixtures; its summary includes feature-detected skips. Inspect that
summary and the exact release SHA before claiming runtime coverage. Adding the
step alone is not evidence that a hosted run passed.

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
./scripts/e2e.sh   # pin/isolation/MCP + dashboard/forensics/goal (feature-detected)
```

- [ ] `CHANGELOG.md` has a dated `## [X.Y.Z]` section with the full feature list
- [ ] Workspace + npm versions match (Cargo.toml, package.json, bin VERSION)
- [ ] Working tree clean (or intentional release commit)
- [ ] Required checks and manual full CI green on the exact release candidate commit
- [ ] e2e green locally (`scripts/e2e.sh`)

## Checklist — v0.5.1 (launch: Oct 20)

1. [x] Bump version → **0.5.1** (Cargo workspace + lock, npm, npm-mcp, apps/web, bin wrapper `VERSION`)
2. [x] `CHANGELOG.md` section for 0.5.1 (npm wrapper hang #64, quickstart → enter #66)
3. [ ] Merge this PR after #66, then tag from clean `main`:
   `git tag -a v0.5.1 -m "Locus v0.5.1" && git push origin v0.5.1`
4. [ ] Release workflow builds the assets; publish `npm/` (`locus-cli`) and `npm-mcp/` (`@ashlrai/locus-mcp`) at 0.5.1
   Each `locus-<target>.tar.gz` ships with a `locus-<target>.tar.gz.sha256`
   sidecar; the workflow fails if any archive's sidecar is missing or wrong.
   Check a download:

   ```bash
   gh release download v0.5.1 -p 'locus-*.tar.gz*' -D /tmp/locus-rel
   (cd /tmp/locus-rel && sha256sum -c locus-*.tar.gz.sha256)   # macOS: shasum -a 256 -c
   ```

   A pull request that changes `release.yml` runs it as a dry run: Linux
   archive, sidecar and checksum verification, no release.
5. [ ] Update `integrations/homebrew/Formula/locus.rb` + the live tap with the v0.5.1 source/asset sha256 (commands below)
6. [ ] Smoke: `npm i -g locus-cli@0.5.1` in a clean HOME → `locus quickstart` → `eval "$(locus hook zsh)"` → `locus enter acme`

## Checklist — v0.5.0

1. [x] Bump version → **0.5.0** (Cargo workspace + npm + bin wrappers)
2. [x] `CHANGELOG.md` section for 0.5.0 (signed adapter-registry release manifests + fail-closed `verify-manifest`, hub MT drop-in `withLocusMcpTenant`, MT conformance e2e, Bearer-only HTTP auth, stdio frame cap, catalog deny annotation, watch heartbeat control-capability findings)
3. [x] Homebrew formula comments: prior source + asset sha256 recorded (see formula)
4. [x] Tag from clean `main` (parent / release owner — **do not force-push tags**):

   ```bash
   git tag -a v0.5.0 -m "Locus v0.5.0"
   git push origin main
   git push origin v0.5.0
   ```

5. [x] Wait for [`.github/workflows/release.yml`](../.github/workflows/release.yml) assets
6. [x] Source tarball sha256 for **v0.5.0** → update formula + live tap:

   ```bash
   curl -sL "https://github.com/ashlrai/locus/archive/refs/tags/v0.5.0.tar.gz" | shasum -a 256
   ```

7. [x] Optional prebuilt digests (after assets land):

   ```bash
   for t in aarch64-apple-darwin x86_64-apple-darwin x86_64-unknown-linux-gnu; do
     curl -sL "https://github.com/ashlrai/locus/releases/download/v0.5.0/locus-$t.tar.gz" | shasum -a 256
   done
   ```

8. [ ] npm: publish `npm/` (`locus-cli`) and `npm-mcp/` (`@ashlrai/locus-mcp`, scoped — `locus-mcp` is third-party-owned on npm) at 0.5.0
9. [x] Verify:

   ```bash
   gh release view v0.5.0
   gh release download v0.5.0 -p 'locus-*.tar.gz' -D /tmp/locus-rel
   locus --version   # or extracted binary
   ```

## Checklist — v0.4.0 (shipped)

1. [x] Bump version → **0.4.0** (Cargo workspace + npm + bin wrappers)
2. [x] `CHANGELOG.md` section for 0.4.0 (multi-tenant MCP multiplexor + grants, Anthropic/OpenAI adapters, `locus switch`, leave --force audit fix, worker-home deletion hardening, …)
3. [x] Homebrew formula comments: prior source + asset sha256 recorded (see formula)
4. [x] Tag from clean `main` (parent / release owner — **do not force-push tags**):

   ```bash
   git tag -a v0.4.0 -m "Locus v0.4.0"
   git push origin main
   git push origin v0.4.0
   ```

5. [x] Wait for [`.github/workflows/release.yml`](../.github/workflows/release.yml) assets
6. [x] Source tarball sha256 for **v0.4.0** → update formula + live tap:

   ```bash
   curl -sL "https://github.com/ashlrai/locus/archive/refs/tags/v0.4.0.tar.gz" | shasum -a 256
   ```

7. [x] Optional prebuilt digests (after assets land):

   ```bash
   for t in aarch64-apple-darwin x86_64-apple-darwin x86_64-unknown-linux-gnu; do
     curl -sL "https://github.com/ashlrai/locus/releases/download/v0.4.0/locus-$t.tar.gz" | shasum -a 256
   done
   ```

8. [ ] npm: publish `npm/` (`locus-cli`) and `npm-mcp/` (`@ashlrai/locus-mcp`, scoped — `locus-mcp` is third-party-owned on npm) at 0.4.0
9. [x] Verify:

   ```bash
   gh release view v0.4.0
   gh release download v0.4.0 -p 'locus-*.tar.gz' -D /tmp/locus-rel
   locus --version   # or extracted binary
   ```

## Checklist — v0.3.0 (shipped)

1. [x] Bump version → **0.3.0** (Cargo workspace + npm + bin wrappers)
2. [x] `CHANGELOG.md` section for 0.3.0 (TTL auto-leave, client add, MCP session pin-anchoring, Grok/Claude wiring, verify_session, control capability, …)
3. [x] Homebrew formula comments: prior source + asset sha256 recorded (see formula)
4. [x] Tag from clean `main` (parent / release owner — **do not force-push tags**):

   ```bash
   git tag -a v0.3.0 -m "Locus v0.3.0"
   git push origin main
   git push origin v0.3.0
   ```

5. [x] Wait for [`.github/workflows/release.yml`](../.github/workflows/release.yml) assets
6. [x] Source tarball sha256 for **v0.3.0** → update formula + live tap:

   ```bash
   curl -sL "https://github.com/ashlrai/locus/archive/refs/tags/v0.3.0.tar.gz" | shasum -a 256
   ```

7. [x] Optional prebuilt digests (after assets land):

   ```bash
   for t in aarch64-apple-darwin x86_64-apple-darwin x86_64-unknown-linux-gnu; do
     curl -sL "https://github.com/ashlrai/locus/releases/download/v0.3.0/locus-$t.tar.gz" | shasum -a 256
   done
   ```

8. [x] npm: publish `npm/` (`locus-cli`) and `npm-mcp/` (`@ashlrai/locus-mcp`, scoped — `locus-mcp` is third-party-owned on npm) at 0.3.0
9. [x] Verify:

   ```bash
   gh release view v0.3.0
   gh release download v0.3.0 -p 'locus-*.tar.gz' -D /tmp/locus-rel
   locus --version   # or extracted binary
   ```

## Checklist — v0.2.0 (shipped)

1. [x] Bump version → **0.2.0** (Cargo workspace + npm + bin wrappers)
2. [x] `CHANGELOG.md` section for 0.2.0 (dashboard, forensics, HTTP MCP, agent, goal, verify, …)
3. [ ] Homebrew formula comments: prior source + asset sha256 recorded (see formula)
4. [ ] Tag from clean `main` (parent / release owner — **do not force-push tags**):

   ```bash
   git tag -a v0.2.0 -m "Locus v0.2.0"
   git push origin main
   git push origin v0.2.0
   ```

5. [ ] Wait for [`.github/workflows/release.yml`](../.github/workflows/release.yml) assets
6. [ ] Source tarball sha256 for **v0.2.0** → update formula + live tap:

   ```bash
   curl -sL "https://github.com/ashlrai/locus/archive/refs/tags/v0.2.0.tar.gz" | shasum -a 256
   ```

7. [ ] Optional prebuilt digests (after assets land):

   ```bash
   for t in aarch64-apple-darwin x86_64-apple-darwin x86_64-unknown-linux-gnu; do
     curl -sL "https://github.com/ashlrai/locus/releases/download/v0.2.0/locus-$t.tar.gz" | shasum -a 256
   done
   ```

8. [ ] npm: publish `npm/` (`locus-cli`) and `npm-mcp/` (`locus-mcp`) at 0.2.0
9. [ ] Verify:

   ```bash
   gh release view v0.2.0
   gh release download v0.2.0 -p 'locus-*.tar.gz' -D /tmp/locus-rel
   locus --version   # or extracted binary
   ```

## Tag and push

Tags must match `v*` so [`.github/workflows/release.yml`](../.github/workflows/release.yml) runs.

```bash
# From the reviewed release commit after its exact-SHA full CI passes:
git tag -a v0.2.0 -m "Locus v0.2.0"
git push origin v0.2.0
```

Annotated tags are preferred (`-a`). Lightweight tags also trigger the workflow.

**Do not move or force-push an existing published tag.** Hotfix → bump patch and re-tag.

### Prior release (v0.1.0) — reference digests

Published 2026-08-07. Useful when validating the formula or comparing assets:

| Artifact | sha256 |
|----------|--------|
| Source `v0.1.0.tar.gz` (GitHub archive) | `a0a8e9e14bd9b3322faca27d2efe42a2dcc473d84a40aab3497a4296b8d68cce` |
| `locus-aarch64-apple-darwin.tar.gz` | `0f184f8f38257ee6b9a623543400ab9ac5b8bc1eb11a105aaf4dc8fe582c3f83` |
| `locus-x86_64-apple-darwin.tar.gz` | `426695db3469c8fe71798268cde6abb3d2dd30b1ad05212682451063b56b08d3` |
| `locus-x86_64-unknown-linux-gnu.tar.gz` | `16632ae69e1881830644ee411c1cb889986d25e1e24e9d8e8b01728bbf0da7c6` |

```bash
gh release download v0.1.0 -R ashlrai/locus -D /tmp/locus-rel --clobber
curl -sL "https://github.com/ashlrai/locus/archive/refs/tags/v0.1.0.tar.gz" | shasum -a 256
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
https://github.com/ashlrai/locus/releases/download/v0.1.1/locus-aarch64-apple-darwin.tar.gz
https://github.com/ashlrai/locus/releases/download/v0.1.1/locus-x86_64-apple-darwin.tar.gz
https://github.com/ashlrai/locus/releases/download/v0.1.1/locus-x86_64-unknown-linux-gnu.tar.gz
```

Source archive (GitHub auto):

```text
https://github.com/ashlrai/locus/archive/refs/tags/v0.1.1.tar.gz
```

## After the tag (Homebrew / npm)

### Homebrew (source formula)

1. Compute source tarball sha256 for the **new** tag (see checklist).
2. Update `integrations/homebrew/Formula/locus.rb`: set `version` and replace the
   placeholder `sha256`.
3. Copy the formula into the live tap (`ashlrai/homebrew-ashlr` or
   `ashlrai/homebrew-locus`) and open a PR. See
   [integrations/homebrew/README.md](../integrations/homebrew/README.md).

### Homebrew (optional prebuilt)

Once release assets exist, switch the formula body to the commented prebuilt
block in `locus.rb` and fill per-target sha256 digests.

### npm wrappers

Bump `VERSION` / `package.json` version in `npm/` and `npm-mcp/` if needed, then
publish from those packages (they download the matching release binary).

## Verify a release

```bash
# GitHub CLI
gh release view v0.1.1
gh release download v0.1.1 -p 'locus-*.tar.gz' -D /tmp/locus-rel

# Local binary
tar -xzf /tmp/locus-rel/locus-aarch64-apple-darwin.tar.gz -C /tmp
/tmp/locus-aarch64-apple-darwin/locus --version
```

## Hotfix / re-tag policy

Do **not** move an existing published tag. Bump patch (`v0.1.2`), document in
CHANGELOG, tag, and push. Never `--force` a tag that already has release assets.
