# CI's role in adapter registry manifests

Release CI currently publishes an **unsigned catalog snapshot**. The CLI can
sign manifests, but CI signing and publication of a registry root are future
work. The [trust guide](./registry-trust.md) explains the format and trust
model; the [v0.5.0 changelog](../CHANGELOG.md#050--2026-08-16) records the
signing capability and the unsigned release attachment.

## What the release workflow does

On a `v*` tag push, the release workflow requires a successful manually
dispatched CI run on the same commit, builds and packages the release binaries,
and checks the archive SHA-256 sidecars. In the release job, it builds and runs
the CLI from the checkout with `--locked`, exporting the embedded built-in
adapter catalog **without** `--sign`. It attaches the resulting
`locus-adapters-<tag>.json` alongside the archives and their sidecars to the
GitHub Release. This export is a separate CLI build, not a manifest extracted
from a packaged binary. The workflow does not run `verify-manifest` on it.

A pull request changing the release workflow triggers a dry run: it builds
the Linux archive, checks the sidecars, and exports an unsigned manifest, but
does not publish a release or npm packages. The dry run does not require the
manual CI evidence used by tag releases; its manifest filename uses the PR ref
with slashes replaced by dashes.

The sidecars check archive bytes against their accompanying digests; they are
not signatures. Someone who can replace an archive and its sidecar can make
that check pass. The manifest does not hash the archives, executables, sidecars,
or npm packages, and it does not bind a tag, commit, or build process. Its
filename alone does not establish any of those relationships.

## What a consumer can check

Obtain the manifest and the corresponding CLI from sources assessed
independently. For example, after substituting the actual release asset name:

```bash
locus adapter verify-manifest release.json --allow-unsigned
```

A matching unsigned manifest reports `signature unsigned` and `drift none`.
This compares catalog metadata with the catalog embedded in the running CLI;
it does not authenticate either file or verify executable code. Without
`--allow-unsigned`, verification rejects an unsigned manifest. The flag does
not waive drift, parsing errors, or an invalid nonempty signature. See the
[consumer walkthrough](./registry-manifest-verification.md) for the checks
and their limits.

## Future signing

The CLI already supports `locus adapter registry export --sign` with an
operator-provided Ed25519 key; operators can sign and distribute manifests
separately from release CI. The commented `TODO(registry-signing)` in the
workflow is an example, not an active step. A CI signing setup, key custody,
and publication of a trusted registry root remain future work; see the
[open questions](./registry-signing-open-questions.md).

A signature on the current format would authenticate its canonical catalog
material to a trusted key holder, but would not by itself bind the manifest to
release artifacts or establish freshness. Consumers still need to establish
the key holder's identity and assess artifact provenance independently.
