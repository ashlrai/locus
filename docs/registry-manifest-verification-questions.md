# Adapter registry manifest verification: open questions

This is a decision aid for the built-in adapter **release manifest**, not the
community adapter registry. The [registry trust guide](./registry-trust.md)
describes the current format and verification limits; the [v0.5.0 changelog
entry](../CHANGELOG.md#050--2026-08-16) records the signing capability and
the unsigned release workflow. The questions below are unresolved; they do
not announce a signing service, trust root, policy, or release commitment.

## What can be verified today?

Release CI exports `locus-adapters-<tag>.json` without `--sign`. There is no
configured CI signing setup or published registry root for this artifact.
The CLI supports optional operator-managed signing, but that separate process
does not make the release asset signed.

The verifier rejects an unsigned manifest by default. Given a manifest and a
corresponding CLI whose sources have been assessed independently, a consumer
can opt into a local catalog comparison:

```bash
locus adapter verify-manifest release.json --allow-unsigned
```

Substitute the actual manifest filename. Success reports `signature unsigned`
and `drift none`: the manifest agrees with the catalog reported by the running
binary under the documented comparison rules. This does not authenticate
either artifact or establish release provenance. The option does not excuse
malformed, untrusted, or invalid nonempty signatures, parsing errors, or drift.
Entry digests cover catalog metadata, not adapter code; archive SHA-256
sidecars do not authenticate their publisher. The manifest does not bind a tag,
commit, build, archive, executable, or npm package, and signing its current
material would not establish freshness. See the [consumer
walkthrough](./registry-manifest-verification.md) for the detailed procedure.

## Questions to resolve before relying on release verification

### Trust and key lifecycle

- Who is authorized to sign a release manifest, and who approves and audits
  access to the signing key? Where and how will the key be generated and held?
- How will consumers independently obtain and authenticate the initial public
  key, and how will its identity and signing authority be communicated?
- How will keys be rotated, revoked, and recovered after compromise? How will
  consumers learn of revocation and decide whether older releases remain valid?
- How will key identifiers, multiple trusted keys, and conflicting local or
  environment trust-store entries be managed without silently changing trust?

### What a verified release should bind

- What exact claim should a signature make, and should a future format bind the
  tag, commit, build identity, target archives, executables, checksums, and npm
  packages? Which artifacts and metadata need independent verification?
- How will consumers detect replay, rollback, or a valid but obsolete manifest?
  Is a trusted timestamp, expiry, or release sequence needed, and who supplies it?
- How will the manifest's schema and canonicalization evolve without ambiguity,
  and how will consumers handle unsupported versions or missing fields?

### Release and consumer operations

- What must CI verify before signing and publishing, and should a signing or
  verification failure block the release? How will the published artifacts be
  checked against the intended source and build?
- What evidence and commands should consumers use to authenticate the CLI and
  release artifacts independently, and how should automated consumers report
  unsigned, unknown-key, invalid-signature, and drift outcomes?
- What is the migration policy for existing unsigned releases and offline
  consumers? When, if ever, should unsigned comparisons be acceptable, and who
  owns that exception decision?
- Who owns the verification policy, incident response, documentation, and
  end-to-end tests, and what evidence is required before announcing CI signing?
