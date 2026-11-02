# Adapter registry trust: CI status

The release workflow currently exports the built-in adapter catalog **without
`--sign`** and attaches the unsigned `locus-adapters-<tag>.json` to a tag-push
release. It does not run `verify-manifest` on that export. There is no
configured CI signing setup or published registry root. See the
[registry trust guide](./registry-trust.md#current-release-status) and the
[v0.5.0 changelog](../CHANGELOG.md#050--2026-08-16): the latter announces
signing support but explicitly describes the CI artifact as unsigned.

## What consumers can check today

Obtain the manifest and the corresponding release CLI from sources assessed
independently. The verifier rejects unsigned manifests by default. To compare
an unsigned file named `release.json` with the catalog in the running CLI:

```bash
locus adapter verify-manifest release.json --allow-unsigned
```

A matching file reports `signature unsigned` and `drift none`. This is a
catalog comparison, not authentication of the manifest, publisher, or binary.
The flag does not waive parsing errors, catalog drift, or a bad nonempty
signature. The manifest does not bind a tag, commit, build, archive, or
executable hash. Separate archive SHA-256 sidecars check archive bytes against
their digests; they do not authenticate the publisher.

## Signing remains future CI work

The CLI can sign a separate manifest with an operator-provided Ed25519 key;
that optional process is independent of release CI. Provisioning a CI signing
key, publishing a trusted registry root, and enabling CI signing remain future
work. The commented signing example in the workflow is a TODO, not an active
step. Even a valid signature on the current format authenticates the catalog
material to a trusted key holder, not the release binaries or their provenance.
See the [signing open questions](./registry-signing-open-questions.md).
