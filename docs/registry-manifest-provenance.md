# Adapter registry manifest provenance

The release manifest is a canonical snapshot of Locus's built-in adapter
catalog. It is **unsigned** in the current release workflow. This page traces
how that attachment is produced and what its origin does and does not establish;
the [registry trust guide](./registry-trust.md) describes the format and trust
model in detail.

## From source to release attachment

1. The catalog starts in [`adapters/manifest.toml`](../adapters/manifest.toml)
   and is compiled into Locus. It describes built-in adapter metadata, not
   downloaded plugins or the implementation of each adapter.
2. For a `v*` tag push, release CI checks out the source and, in the release
   job, builds and runs `locus-cli` with `cargo run --release --locked`. The
   command `locus adapter registry export` produces the canonical JSON from
   that CLI's embedded catalog and workspace version. Adapter and tool lists
   are sorted, so repeated exports from the same binary are byte-identical.
   This CLI build is separate from the binaries in the release archives.
3. The same job attaches the result as `locus-adapters-<tag>.json` to the
   GitHub Release, alongside the archives and their SHA-256 sidecars. A pull
   request changing the release workflow exercises an export as a dry run but
   does not publish a release.

See [CI's role](./registry-manifest-ci-role.md) for the release gate and dry
run details. This describes the configured workflow, not independent proof
that a particular downloaded file was produced by that workflow.

## Signing and evidence

CI exports **without `--sign`**; no CI signing setup or published registry
root is provided for this artifact. Signing in CI and publishing a registry
root are future work. The CLI supports an operator signing and distributing a
separate manifest with an Ed25519 key, but that is not the current release
process. See the [trust model](./registry-trust.md#trust-model).

The manifest is catalog metadata, not a signed provenance statement. Its tag
in the filename does not bind it to a tag, commit, build process, archive, or
executable. Entry digests cover specified catalog metadata, not adapter code;
the manifest does not hash the archives, executables, sidecars, or npm packages.
The sidecars check archive bytes, but do not authenticate their publisher.

By default `locus adapter verify-manifest` rejects an unsigned manifest.
`--allow-unsigned` permits a matching unsigned catalog comparison against the
running CLI; it does not authenticate either file or waive drift, parsing
errors, or an invalid nonempty signature. Assess their sources independently.
Even an operator-signed manifest authenticates only its canonical catalog
material to a trusted key holder, not the binary, release provenance, or
freshness. See the [verification walkthrough](./registry-manifest-verification.md)
for the command and its limits.
