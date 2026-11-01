# Adapter registry manifest: current state

The release asset `locus-adapters-<tag>.json` is an **unsigned** snapshot of
the built-in adapter catalog. The release workflow runs `locus adapter
registry export` without `--sign`. It has no configured CI signing setup or
published registry root; enabling release signing and publishing a root are
**unimplemented future work**, not properties of current releases. The
[trust guide](./registry-trust.md) describes the workflow and trust model;
the [v0.5.0 changelog entry](../CHANGELOG.md#050--2026-08-16) records both
the signing capability and the unsigned release process.

## What the manifest contains

The `locus-adapter-registry/v1` JSON describes the catalog compiled into the
exporting CLI: the Locus workspace version and, for each adapter, its ID,
name, version, sorted tool names, and a SHA-256 digest of canonical entry
metadata. The digest covers status, synthetic flag, capabilities, frozen
selectors, tools, and destructive tools as well as ID and name. It excludes
the entry description and per-entry signature and signer. It is not a hash
of adapter implementation or executable code.

The tag appears in the filename, not as an authenticated tag or commit
binding. The manifest does not hash release archives, executables, checksum
sidecars, or npm packages, and does not describe the build process. Archive
SHA-256 sidecars separately check archive bytes, but do not authenticate
their publisher; replacement of both an archive and its checksum can pass
that check.

## Checking today's asset

Obtain the manifest and a same-version Locus binary from sources you assess
independently, then substitute the actual filename:

```bash
locus adapter verify-manifest release.json --allow-unsigned
```

The command reads the local file and compares its catalog with the catalog
compiled into the binary running the command. A match reports `signature
unsigned` and `drift none`. Without `--allow-unsigned`, the verifier rejects
the unsigned asset. The flag permits only an absent or empty signature; it
does not waive parsing errors, catalog drift, or a nonempty invalid, malformed,
or untrusted signature. See the [verification walkthrough](./registry-manifest-verification.md)
for details.

This comparison establishes catalog agreement under the verifier's rules,
not provenance or publisher identity for the manifest or binary. It does not
verify the release artifacts, detect code changes that leave catalog metadata
unchanged, or establish freshness. Independently assess the binary and any
release artifacts before relying on them.

## Signing capability and future release signing

The CLI already supports an operator signing a separate manifest with an
operator-provided Ed25519 key and verifying it against an independently
trusted public key. This optional process is not the release workflow. A valid
signature authenticates the signed catalog material to the key holder; it
still does not bind a tag, commit, build, or artifact, authenticate the
binary, or prevent replay. Key provisioning, root publication, and enabling
CI signing remain unimplemented; the commented workflow example is a TODO.
See the [open signing questions](./registry-signing-open-questions.md) for
unresolved decisions.
