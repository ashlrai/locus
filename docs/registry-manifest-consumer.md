# Consuming the adapter registry release manifest

This page is for a consumer who has downloaded `locus-adapters-<tag>.json`
from a Locus release and wants to know what it is worth. It is derived from
[registry-trust.md](./registry-trust.md), which is the authoritative source;
if the two disagree, that page wins. For a pre-flight list see the
[consumer checklist](./registry-manifest-consumer-checklist.md); for common
questions see the [registry trust FAQ](./registry-trust-faq.md).

## Current status: unsigned

The release workflow runs `locus adapter registry export` **without**
`--sign` and attaches the resulting manifest to the tag-push release. The
asset therefore carries no signature. There is no configured CI signing setup
and no published registry root for it. Signing in CI is future work, not a
current release guarantee, and nothing on this page should be read as a
promise of when it will exist.

The archive `.sha256` sidecars are a separate mechanism. They check archive
bytes for integrity; they do not authenticate a publisher and do not sign or
reference the manifest.

## What the manifest is

A canonical JSON snapshot of the built-in adapter catalog compiled into the
Locus CLI that produced it: the schema id, the workspace version, and for each
adapter its id, name, version, tool names, and a `sha256` digest of the entry's
catalog metadata. It is catalog metadata, not a signed provenance statement.

## What you can verify

Run the verifier locally. It reads the file and compares it with the catalog
compiled into **the binary you run**; it downloads nothing.

```bash
locus adapter verify-manifest locus-adapters-v0.5.2.json --allow-unsigned
```

Replace `v0.5.2` with your release tag. A success reports `signature unsigned`
and `drift none`. That establishes:

- The file parses and uses a supported schema, with no empty or duplicate
  adapter ids.
- The Locus version, adapter ids, names, adapter versions, tool lists, and
  per-entry digests match the running binary's catalog (ids compared
  case-insensitively, tool order ignored).
- Any added, missing, or changed entry is reported as drift and fails.

Without `--allow-unsigned` the command fails on this asset even when the
catalog matches. The flag relaxes only a missing or empty signature: a nonempty
malformed signature, drift, and unparseable JSON still fail.

## What you cannot verify

| Question | Answer today |
|----------|--------------|
| Who published the manifest? | Not established. There is no signature to check. |
| Was it modified after publication? | Not detectable. An attacker who can replace it can supply a matching one. |
| Does it belong to a given tag, commit, or build? | No. The filename names a tag; the contents do not bind one. |
| Does it describe a given archive or executable? | No. It holds no hashes of archives, sidecars, `locus`, `locus-mcp`, or npm packages. |
| Is the binary authentic or unmodified? | No. A tampered binary can export and verify its own matching manifest. |
| Is the adapter code unchanged? | No. Digests cover catalog metadata, not implementation. |
| Is it current? | No freshness guarantee; an old matching manifest remains valid. |

A matching comparison therefore means only that this file and this binary agree
about the catalog. Establish the binary's provenance and the archive's origin
through independent trusted channels.

## Operator-signed manifests

An operator may sign a manifest locally (`locus adapter registry export
--sign`) and distribute their public key out of band. A consumer who pins that
key with `locus adapter trust add` can then verify the signature without
`--allow-unsigned`. That authenticates the catalog material to the key holder
only, and is not the CI release process. Release manifests accept ed25519
signatures only; HMAC signatures are reported as malformed.
