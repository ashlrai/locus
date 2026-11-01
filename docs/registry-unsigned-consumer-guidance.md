# Consuming an unsigned adapter registry manifest

The release asset `locus-adapters-<tag>.json` is an **unsigned** snapshot of
the built-in adapter catalog, not an authenticated release statement. The
release workflow exports it without `--sign`; CI signing and a published
registry root are future work. Locus supports separate operator signing, but
that is not the current release process. See the
[registry trust guide](./registry-trust.md) for the format and trust model.

## Decide whether the check is enough

Use the unsigned manifest to look for catalog drift, not as the sole basis
for trusting a release. If your policy requires authenticated provenance or
artifact integrity, an unsigned comparison alone cannot satisfy it. Obtain
and assess the manifest and the corresponding Locus CLI independently through
sources your organization trusts; use the same release version for both.

Run the following locally, replacing the example filename with the actual
asset name:

```bash
locus adapter verify-manifest locus-adapters-v0.6.2.json --allow-unsigned
```

The command reads the file locally and compares it with the catalog compiled
into **the CLI running the command**. A matching unsigned manifest reports
`signature unsigned` and `drift none` and exits successfully. Without
`--allow-unsigned`, the unsigned manifest is rejected. The flag permits only
an absent or empty signature; parsing errors, catalog drift, and nonempty
invalid, malformed, or untrusted signatures still fail. Investigate a nonzero
exit instead of treating it as a successful check. `--json` provides a
machine-readable report.

## Interpret the result

A successful check establishes agreement under the verifier's comparison
rules: the Locus version, adapter IDs, names, adapter versions, tool lists,
and per-entry metadata digests match the running CLI's catalog. IDs are
matched case-insensitively, surrounding whitespace is trimmed, and tool
order is ignored. Entry digests cover canonical catalog metadata, not adapter
implementation code.

That result does **not** establish:

- The manifest's publisher, origin, or integrity against tampering; a
  `signed_by` value on an unsigned manifest is an unauthenticated claim.
- The authenticity or provenance of the CLI, or that it came from the named
  release. A substituted CLI and a manifest exported from it can agree, and
  changed executable behavior can leave the catalog unchanged.
- A binding to a tag, commit, build process, or time; a version or filename
  is not such a binding, and a matching old manifest can be replayed.
- Integrity or provenance of release archives, executables, sidecars, packaged
  documents, or npm packages. The manifest does not hash these artifacts.
  Archive SHA-256 sidecars are separate byte-integrity checks; someone who can
  replace both an archive and its sidecar can make that check pass.

For a separately operator-signed manifest, establish the key holder and their
authorization through a trusted channel and verify using the pinned public key.
Even then, a valid signature authenticates catalog material to the key holder,
not the binary or archive, and does not establish freshness. The
[verification walkthrough](./registry-manifest-verification.md) and
[consumer checklist](./registry-manifest-consumer-checklist.md) provide further
details.
