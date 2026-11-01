# Unsigned adapter registry manifest FAQ

This FAQ covers the release manifest for Locus's **built-in adapter catalog**.
The detailed source of truth is [adapter registry release manifests](./registry-trust.md).

## What is the release manifest, and why is it unsigned?

The tag-push release workflow builds a CLI, exports its embedded adapter catalog
without `--sign`, and attaches the resulting `locus-adapters-<tag>.json` to the
release. It describes adapter IDs, names, versions, tools, and catalog metadata
digests. The workflow has no configured signing setup or published registry root
for this artifact. A filename containing a tag does not bind the manifest to
that tag or a commit.

## How do I compare it with a Locus CLI?

Independently assess the sources of the downloaded manifest and the CLI from
the same release version, then run the following locally, replacing
`release.json` with the manifest's path:

```bash
locus adapter verify-manifest release.json --allow-unsigned
```

Without `--allow-unsigned`, the verifier rejects an unsigned manifest. With
the flag, a match reports `signature unsigned` and `drift none`: the manifest
agrees with the catalog compiled into **the CLI running the command**, under
the [comparison rules](./registry-trust.md#what-verify-manifest-proves). The
flag only permits an absent or empty signature; malformed input, catalog drift,
and nonempty invalid, malformed, or untrusted signatures still fail. Investigate
any nonzero exit.

## Does a match prove that the release is authentic?

No. The unsigned comparison does not authenticate the manifest, its publisher,
or the CLI. A substituted CLI and a matching manifest can pass together. The
catalog digests do not hash adapter code, and the manifest does not bind the
release tag, commit, build process, archives, or executables. The archive
SHA-256 sidecars check archive bytes separately; they do not authenticate their
publisher, and replacing both an archive and its sidecar can defeat that check.
Assess artifact sources independently before relying on them.

## Can someone sign a manifest today?

Yes. An operator can separately sign a manifest with an Ed25519 private seed
and distribute the corresponding public key through a trusted channel. A
consumer who independently establishes the key holder and pins the key can
verify the signed catalog and check for drift. This optional operator process
is not the release CI process. Even a valid signature does not authenticate
the executable, bind the manifest to a release artifact, or establish freshness.
See the [operator and consumer commands](./registry-trust.md#commands).

## What is future work?

Provisioning a CI signing key, publishing its public key as a registry root,
and enabling CI signing are future work, not current release guarantees. The
commented workflow example is a TODO. Consumers would still need to establish
trust in the published key and verify the signature; signing the current format
would authenticate its catalog material without adding artifact hashes or
freshness guarantees. See the [open signing questions](./registry-signing-open-questions.md).
