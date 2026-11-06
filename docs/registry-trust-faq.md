# Adapter registry trust FAQ

This FAQ concerns the **release manifest** for the built-in adapter catalog,
not the MCP Registry listing or a plugin loader. The detailed
[registry trust guide](./registry-trust.md) describes the format and trust
model; the [0.5.0 registry-signing note](../CHANGELOG.md#050--2026-08-16)
records the feature and the release workflow's unsigned status.

## Are release manifests signed today?

No. Release CI exports `locus-adapters-<tag>.json` without `--sign`. There is
no configured CI signing setup or published registry root for this artifact.
The signing example in the workflow is a future plan, not an active release
guarantee. The filename names a tag but does not bind the manifest to that tag
or a commit.

## What can I check with an unsigned manifest?

By default, `verify-manifest` rejects it. To opt in to a local, drift-only
comparison, obtain the manifest and the corresponding Locus CLI, assess their
sources independently, and substitute the actual file name here:

```bash
locus adapter verify-manifest release.json --allow-unsigned
```

A successful comparison reports `signature unsigned` and `drift none`: the
manifest's catalog matches the catalog reported by the running binary under
the documented comparison rules. It does not authenticate either file or
establish their provenance. The flag excuses only an absent or empty
signature; malformed, untrusted, or invalid signatures, parsing errors, and
catalog drift still fail. See the [consumer walkthrough](./registry-manifest-verification.md).

## Do the entry digests or archive checksums prove the release is authentic?

No. Entry digests cover canonical catalog metadata, not adapter code. The
manifest does not hash release archives, executables, checksum sidecars, or
npm packages, and does not bind a build process. Archive SHA-256 sidecars
check whether archive bytes match the sidecar; an attacker who replaces both
can make that check pass. Assess the binary and other artifacts independently.

## Can an operator sign a manifest now?

Yes, separately from CI. An operator may sign with an Ed25519 private key and
distribute the public key through a trusted channel. Consumers must establish
the key holder's identity and authority and pin the public key in their local
trust store. Verification then requires both a valid signature and no catalog
drift. It authenticates the signed canonical catalog material to the key
holder; it does not authenticate the running binary, archive, tag, commit, or
build, and does not establish freshness. A `signed_by` name alone is a claim,
not proof. See the [signing and verification commands](./registry-trust.md#commands).

## What changes at the future CI signing boundary?

Future work includes provisioning a signing key, publishing its public key as a
registry root, and enabling CI signing. Consumers would still need to obtain
and trust that public key independently and verify the signature. Signing the
current manifest format would authenticate its canonical catalog material,
not the release artifacts or their provenance, and would not prevent replay.
No timeline or current CI signing guarantee is implied. The unresolved
decisions for that transition are tracked in
[registry signing open questions](./registry-signing-open-questions.md).
