# Adapter registry signing: open questions

> Decision aid for the **unsigned → signed** transition of the adapter
> registry release manifest. Every item below is unresolved; nothing here
> announces a signing service, a registry root, a timeline, or a release
> commitment.

This page tracks the **producer-side** signing decisions. For the current
format and trust model see [registry-trust.md](./registry-trust.md); for
what a consumer can check today see
[registry-manifest-verification.md](./registry-manifest-verification.md);
for consumer-side reliance questions see
[registry-manifest-verification-questions.md](./registry-manifest-verification-questions.md).
The [v0.5.0 changelog entry](../CHANGELOG.md#050--2026-08-16) records the
signing capability and the unsigned release workflow, and the
[signed registry roadmap](./adapter-sdk.md#signed-registry-roadmap) lists
CI signing as roadmap work.

## What exists today

Two different things are easy to conflate:

- **Signing capability in the CLI (landed).** `locus adapter registry
  export --sign` signs a manifest with an operator-provided ed25519 key
  (`--key <file>` or `LOCUS_REGISTRY_SIGNING_KEY`; the seed is never
  generated or printed). `locus adapter verify-manifest` checks a
  signature against the local trust store and is fail-closed;
  `--allow-unsigned` relaxes only a missing signature for a drift-only
  comparison. An operator can run this process today, out of band — see
  the [commands](./registry-trust.md#commands).
- **Release signing deployment (not present).** Release CI exports
  `locus-adapters-<tag>.json` **without** `--sign`. The
  `TODO(registry-signing)` block in the release workflow is a commented
  example, not an active step; no `LOCUS_REGISTRY_SIGNING_KEY` repository
  secret is provisioned and no registry root public key is published. The
  release asset is unsigned catalog metadata, and `verify-manifest`
  rejects it by default.

The check a consumer can run today against that unsigned asset is a
drift-only comparison against the running binary, substituting the actual
asset name:

```bash
locus adapter verify-manifest release.json --allow-unsigned
```

A matching asset reports `signature unsigned` and `drift none`. It
authenticates neither the manifest nor the binary; see the
[trust guide](./registry-trust.md) for the full limit list.

## What future signing would — and would not — change

Signing the **current** manifest format would authenticate its canonical
catalog material (locus version, adapter ids, names, versions, tool
lists, and entry digests) to the holder of the signing key, provided the
consumer already trusts the matching public key. By itself it would not:

- bind the manifest to a tag, a commit, or a build process;
- cover release archives, executables, `.sha256` sidecars, or npm
  packages;
- establish freshness or detect replay of an older matching catalog;
- authenticate the binary a consumer uses to run the check.

Each gap is a design decision below, not something to patch silently at
release time.

## Open questions

### Signing key custody and provisioning

- Who generates the ed25519 seed, on what machine, under whose approval,
  and with what audit trail? Options include a CI repository secret, a
  cloud KMS/HSM, or an offline key used to sign out of band.
- The workflow comment anticipates a base64 or 64-hex seed in
  `LOCUS_REGISTRY_SIGNING_KEY`. Is an env-injected raw seed the intended
  production shape, or should signing live behind an opaque signer that
  never exposes key material to the job runner?
- Which environment protections gate the signing job (protected
  environment, required reviewers, tag-only triggers), and who may
  rotate, suspend, or destroy the key after a suspected compromise?

### Registry root publication and trust bootstrap

- Where does the public key live so consumers can authenticate it
  independently — release notes, this repository's docs, a well-known pin
  shipped in the binary, or several at once? The roadmap calls for a
  well-known pin, yet production currently ships no baked-in keys; is
  that still the intent, and who approves changing it?
- What key id does the root use (`root`, `locus-registry-root`, other),
  and how are multiple concurrent roots named during a rotation overlap?
- How do consumers detect conflicts between the file trust store and the
  `LOCUS_ADAPTER_TRUST_KEYS` env overlay instead of silently shifting
  trust when entries disagree?

### What a future signature should claim

- Does `locus-adapter-registry/v1` gain an optional claim extension (tag,
  commit, builder identity, artifact digests, timestamp), or does a new
  schema version carry them? How does canonicalization evolve without
  ambiguity for older verifiers?
- Which artifacts, if any, should the manifest enumerate — archives,
  executables, sidecars, npm packages — and which claims stay explicitly
  out of scope so consumers keep their independent verification duties?

### Freshness, rotation, and revocation

- How are replay and rollback addressed: a trusted timestamp, an expiry,
  a monotonic release sequence, or an append-only transparency log? Who
  supplies that anchor?
- What is the rotation story: overlap window, how `signed_by` selects the
  successor key, whether older releases remain verifiable, and how
  revocation reaches consumers without soft-allowing unknown keys?

### CI integration and release policy

- What preconditions gate enabling the commented signing step — secret
  provisioning, root publication, consumer docs, and an end-to-end test
  of a signed release artifact?
- Does a signing failure, or a failed post-export `verify-manifest`
  self-check, block the release, or ship unsigned with a notice? Who
  decides, and how is the decision recorded?
- Should CI run `verify-manifest` against the freshly exported manifest
  before attaching it, and against which trust material?

### Rollout and consumer migration

- Are existing unsigned releases left as-is permanently, backfilled with
  signatures, or superseded under a documented grandfathering scheme?
- Does `--allow-unsigned` remain supported indefinitely, gain a
  deprecation window, or become warning-on-use once signed assets exist?
- What should automated consumers do with mixed signed and unsigned
  history, and who owns the exception decision for unsigned comparisons?

### Ownership and evidence bar

- Who owns the transition end to end — key ceremony, workflow change,
  documentation, tests, and the announcement — and what evidence, such as
  a verified signed manifest on a real tag-push release, must exist
  before these pages stop calling signing future work?

## Scope notes

- A CLI capability is not a deployment claim. Until a provisioned key, a
  published root, and an enabled CI step exist together, release
  manifests remain unsigned and the verification limits in
  [registry-trust.md](./registry-trust.md) apply unchanged.
- Per-entry catalog signatures (`locus adapter verify
  [--require-signed]`) and the optional remote index are separate
  surfaces covered by the [adapter SDK](./adapter-sdk.md#signature-schemes);
  their open decisions are not repeated here.
