# Current adapter registry manifest guarantees

The release asset `locus-adapters-<tag>.json` is an **unsigned snapshot of
the built-in adapter catalog**, not a signed release attestation. The current
release workflow exports it without `--sign`; there is no configured CI
signing setup or published registry root for this artifact. The
[registry trust guide](./registry-trust.md#current-release-status) describes
the current workflow and coverage; the [0.5.0 changelog entry](../CHANGELOG.md#050--2026-08-16)
records both the signing feature and the unsigned CI asset. This page concerns
that adapter catalog, not the MCP Registry listing.

## What the asset and check cover today

- The manifest describes the catalog embedded in the exporting CLI and its
  workspace version. Adapter rows include IDs, names, versions, sorted tool
  names and SHA-256 digests of canonical entry metadata. The digest covers
  status, synthetic flag, capabilities, frozen selectors and destructive tools
  as well as ID, name and tools; it does not cover the description, per-entry
  signature or signer, or adapter implementation.
- `locus adapter verify-manifest` rejects an unsigned manifest by default.
  Obtain the manifest and a CLI for the same release, assess their sources
  independently, then explicitly opt into a local comparison (substitute the
  actual filename):

  ```bash
  locus adapter verify-manifest release.json --allow-unsigned
  ```

  Success reports `signature unsigned` and `drift none`: the manifest agrees
  with the catalog in **the binary running the check**, under the comparison
  rules. IDs are matched case-insensitively, surrounding whitespace is
  trimmed, and tool order is ignored. Version, adapter IDs, names, versions,
  tool lists and entry digests are compared. Parsing errors and drift fail;
  the flag only permits an absent or empty signature, and does not excuse a
  nonempty malformed, untrusted or invalid signature.
- This comparison does not authenticate either file or prove the binary's
  behavior. A replacement binary and a manifest exported from it can agree;
  code changes that leave catalog metadata unchanged are also invisible.
  A `signed_by` value without a valid signature is only a claim.

The filename names a tag, but the manifest does not bind a tag, commit, build
process, archive or executable hash. It does not cover npm packages. Separate
archive SHA-256 sidecars check archive bytes against the sidecar; they do not
authenticate the publisher, and replacing both archive and sidecar can make
that check pass. See the [verification walkthrough](./registry-manifest-verification.md)
for the separate checks and their limits.

## Optional signing and future release work

An operator can sign a separate manifest now with an Ed25519 key and distribute
the public key through a trusted channel. With that key pinned, verification
requires a valid signature **and** no catalog drift. The signature authenticates
the canonical catalog material to the key holder; consumers must establish the
holder's identity and authority independently. It does not authenticate the
binary or archive, bind a tag or build, or establish freshness.

Provisioning a CI signing key, publishing its public key as a registry root,
and enabling CI signing are **future work**, not current release guarantees.
Signing the current manifest format would not by itself add artifact hashes,
release provenance or replay protection. See the
[trust model and signing commands](./registry-trust.md#trust-model) and the
[signed registry roadmap](./adapter-sdk.md#signed-registry-roadmap).
