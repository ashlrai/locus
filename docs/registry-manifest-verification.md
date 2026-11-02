# Verifying an adapter registry release manifest

The current release asset `locus-adapters-<tag>.json` is **unsigned**. It is
a snapshot of the built-in adapter catalog, not a signature on a release. CI
exports it without `--sign`; no release registry signing key or published
registry root is configured. For the format, trust store, and optional
operator signing procedure, see [registry-trust.md](./registry-trust.md).
Provisioning a release signing key, publishing its public key as a registry
root, and enabling CI signing remain future work.

## Check an unsigned manifest

1. Obtain the manifest and a Locus CLI for the same release version. Assess
   the source of each independently: the command compares the manifest with
   the catalog embedded in **the binary running the check**.
2. Run the local comparison, substituting the actual asset name:

   ```bash
   locus adapter verify-manifest locus-adapters-v0.6.2.json --allow-unsigned
   ```

   Replace `v0.6.2` with the actual release tag and quote the filename if it
   contains shell-special characters. Add `--json` for a machine-readable
   report. A matching unsigned asset reports `signature unsigned` and
   `drift none` and exits zero. Without `--allow-unsigned`, an unsigned asset
   fails even if the catalog matches. A nonzero exit requires investigation.
3. The parser rejects invalid JSON, an unsupported schema, empty or duplicate
   adapter IDs, and ambiguous canonical-material separators. The drift check
   compares the Locus version and adapter IDs, names, versions, tool lists, and
   entry digests against the running binary. It trims surrounding whitespace,
   matches IDs case-insensitively, and ignores tool order. Added, missing, or
   changed entries fail. `--allow-unsigned` waives neither parsing errors nor
   drift, and a nonempty malformed, untrusted, or invalid signature still fails.

The entry digests are SHA-256 hashes of canonical catalog metadata, including
capabilities and frozen selectors. They are compared to the binary's catalog;
they are not hashes of adapter code. Agreement does not prove that the binary
is authentic or unmodified. A replacement binary and a manifest exported from
it can agree, and changed behavior with unchanged catalog metadata is invisible
to this check. A `signed_by` value on an unsigned manifest is only a claim.

## Check an archive separately

Release archives have separate `.sha256` sidecars. From the directory
containing both the archive and its corresponding sidecar, for example:

```bash
sha256sum -c locus-x86_64-unknown-linux-gnu.tar.gz.sha256
```

On systems with `shasum` instead, use
`shasum -a 256 -c locus-x86_64-unknown-linux-gnu.tar.gz.sha256`. Replace the
example filename with the actual sidecar. This checks that the archive bytes
match the digest in the sidecar; it does not authenticate either file. Someone
who can replace both can make the check pass. The manifest neither references nor
checks archives, executables, sidecars, or npm packages, and does not bind a
tag, commit, or build process. Obtain and assess artifacts through trusted
channels independently.

## Signature verification is a separate check

An operator can separately sign a manifest with Ed25519 and distribute the
public key through a trusted channel. After pinning that key in the local
trust store, `locus adapter verify-manifest <file>` requires both a valid
signature and no catalog drift. A valid signature authenticates the canonical
manifest material to the holder of the pinned key; the consumer must establish
the key holder's identity and authority. It does not authenticate the binary
or archive, bind a release tag, or establish freshness. Release manifest
signatures accept Ed25519 only; per-entry HMAC verification is a different
facility. See [registry-trust.md](./registry-trust.md) for setup and details.
