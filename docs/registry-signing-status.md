# Adapter registry signing status

## Current releases

The release workflow exports `locus-adapters-<tag>.json` **without `--sign`**.
This is an **unsigned** snapshot of the built-in adapter catalog; CI has no
configured signing setup or published registry root. The
[registry trust guide](./registry-trust.md#current-release-status) describes
the current release process. The [v0.5.0 changelog entry](../CHANGELOG.md#050--2026-08-16)
records both the signing capability and the unsigned CI artifact, despite its
“Signed adapter-registry release manifests” heading.

By default, `locus adapter verify-manifest` rejects this unsigned asset. With
`--allow-unsigned`, it checks catalog agreement with the running binary; it
does not authenticate the manifest, publisher, or binary. The flag does not
accept a nonempty invalid signature or ignore catalog drift. The manifest does
not bind a tag, commit, build, executable, or archive hash. Separate archive
SHA-256 sidecars check archive bytes, but do not authenticate their publisher.
See the [verification walkthrough](./registry-manifest-verification.md) for
the procedure and its limits.

After independently assessing the sources of the manifest and corresponding
release CLI, a consumer can compare the catalog locally:

```bash
locus adapter verify-manifest release.json --allow-unsigned
```

Here `release.json` is the downloaded manifest, and the CLI should be from the
same release version. A match reports `signature unsigned` and `drift none`;
this is catalog agreement, not a verified signature or proof of provenance.

## Available capability and future work

An operator can already sign a separate manifest with an operator-provided
Ed25519 key and verify it using an independently trusted public key. This is
an optional operator-managed process, not the release workflow. A valid
signature authenticates the signed catalog material to the key holder; it
does not authenticate the executable or prove release provenance or freshness.
The consumer must independently establish the key holder's identity and
authority to sign.
See the [trust model](./registry-trust.md#trust-model) and
[verification guarantees](./registry-trust.md#what-verify-manifest-proves).

Provisioning a CI signing key, publishing a registry root, and enabling CI
signing are **future work**. The commented workflow example is a TODO, not
an active signing step. Key distribution, rotation, and the transition to
signed releases remain open decisions; see the
[signing open questions](./registry-signing-open-questions.md).
