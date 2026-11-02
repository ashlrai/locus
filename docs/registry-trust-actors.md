# Adapter registry trust actors

This guide maps the parties in the [registry trust model](./registry-trust.md#trust-model)
to their responsibilities. These are roles in a trust process, not necessarily
different people or organizations. The current release asset is **unsigned**;
the operator signing flow is optional and separate from release CI.

| Actor | Responsibility | Source |
|-------|----------------|--------|
| **CI** | Builds the CLI from the checkout, exports its embedded catalog without `--sign`, and attaches the unsigned manifest to the tag release. The archive SHA-256 sidecars check archive bytes but do not authenticate their publisher. CI does not currently use a signing key for the export; a CI signing setup and published registry root are future work. | [Current release status](./registry-trust.md#current-release-status), [Artifact coverage](./registry-trust.md#artifact-coverage), [Trust model](./registry-trust.md#trust-model) |
| **Operator** | If choosing to sign separately, supplies and protects an Ed25519 private seed, signs a manifest, and distributes it and the matching public key through a trusted channel. This is not the current CI release process. | [Trust model](./registry-trust.md#trust-model), [Operator commands](./registry-trust.md#operator-optionally-sign-and-distribute-a-manifest) |
| **Consumer** | Independently assesses the sources of the manifest and running binary. For operator-signed manifests, independently obtains and pins the public key, establishes who controls it and what they may sign, and verifies the manifest against the running binary. Investigates parsing, signature, and drift failures. | [Local verification](./registry-trust.md#local-verification-of-an-unsigned-release-manifest), [Trust model](./registry-trust.md#trust-model), [What verification proves](./registry-trust.md#what-verify-manifest-proves) |

## Boundaries between the roles

- A consumer may explicitly use `--allow-unsigned` to compare an unsigned
  manifest with the running binary's catalog. Success establishes catalog
  agreement under the comparison rules, not the provenance or authenticity
  of either file. The flag does not waive parsing errors, drift, or a bad
  nonempty signature. See [local verification](./registry-trust.md#local-verification-of-an-unsigned-release-manifest).
- A valid signature authenticates canonical catalog material to the holder of
  the locally trusted key. The consumer still needs independent evidence of
  key identity, authorization, and binary provenance. The manifest does not
  bind a tag, commit, archive, or executable hash, and a signature does not
  establish freshness. See [what verification proves](./registry-trust.md#what-verify-manifest-proves)
  and [artifact coverage](./registry-trust.md#artifact-coverage).
- The release manifest uses Ed25519 only. HMAC trust keys are for per-entry
  catalog verification and cannot authenticate release manifests. See the
  [trust model](./registry-trust.md#trust-model).
