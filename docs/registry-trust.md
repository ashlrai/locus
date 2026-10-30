# Adapter registry release manifests

> Companion to the [Adapter SDK](./adapter-sdk.md) "Signed registry roadmap".
> This page covers the **release manifest**: a per-release, signable snapshot of
> the built-in adapter set.

## Current release status

The release workflow runs `locus adapter registry export` without `--sign`
and attaches the resulting **unsigned** `locus-adapters-<tag>.json` to a
tag-push release. CI signing is **not active**: the workflow describes it as
a TODO contingent on provisioning a registry signing key and publishing its
public key. The archive SHA-256 sidecars check the archives for integrity;
they do not authenticate their publisher or sign the manifest.

By default, `verify-manifest` rejects the unsigned release asset. Passing
`--allow-unsigned` checks for drift against the running binary but does not
establish the manifest's provenance. An operator can separately sign and
distribute a manifest; that optional process is not the CI release process.
Publishing a registry root and signing manifests in CI remain future plans,
not guarantees of current releases. See the [roadmap](./adapter-sdk.md#signed-registry-roadmap).

The attachment is catalog metadata, not a signed provenance statement. Its
filename names the tag, but the manifest does not bind a Git tag or commit,
build process, archive, or executable hash. A version string is not proof of
any of those relationships. An attacker able to replace both an archive and
its checksum sidecar can make the checksum check pass.

### Artifact coverage

The release job builds a CLI from the checked-out source and exports one
manifest from its embedded `adapters/manifest.toml` catalog. The manifest
describes that catalog and the workspace version, rather than enumerating
release files or target platforms. Its adapter rows contain the ID, name,
workspace version, tool names, and a digest of the entry metadata listed below.
The digest does not include the catalog entry's description or its per-entry
signature and signer. Those fields are not compared by `verify-manifest`.

The manifest contains no hashes for the platform archives, their `.sha256`
sidecars, the `locus` and `locus-mcp` executables inside them, or the README
and license when packaged alongside them. It also does not cover the npm packages
published by the release workflow. The archive sidecars separately hash the
final archive bytes; they are not referenced or verified by the manifest.
`verify-manifest` reads the JSON and compares it with the catalog in the CLI
that runs the command, without opening any of those release artifacts. Even a
successful unsigned comparison therefore establishes only catalog agreement
under the comparison rules, not artifact integrity or provenance.

## Local verification of an unsigned release manifest

1. Obtain the release's `locus-adapters-<tag>.json` and the corresponding
   Locus binary. Use the same release version for both; the command compares
   the manifest with the catalog compiled into **the binary you run**. Assess
   their sources independently before relying on either artifact.
2. Run the verifier locally, explicitly opting in to an unsigned manifest:

   ```bash
   locus adapter verify-manifest locus-adapters-v0.5.2.json --allow-unsigned
   ```

   Replace `v0.5.2` with the actual release tag and use a quoted filename if
   needed. The command reads the file locally; it does not download artifacts.
   Add `--json` for a machine-readable report. A matching unsigned manifest
   reports `signature unsigned`, `drift none`, and exits successfully. Without
   `--allow-unsigned`, it fails even when the catalog matches.
3. Investigate any nonzero exit and the reported findings. Parsing rejects
   invalid JSON, an unsupported schema, empty or duplicate adapter IDs, and
   fields containing ambiguous canonical-material separators. The drift check
   compares the Locus version, adapter IDs, names, adapter versions, tool lists,
   and per-entry digests with the running binary. It reports added, missing, or
   changed entries. The comparison rules are described below. A nonempty bad
   signature still fails with `--allow-unsigned`; the flag does not waive drift
   or parsing errors.

This establishes catalog agreement only. An unsigned manifest cannot authenticate
its publisher, origin, or integrity against tampering, and agreement cannot
authenticate the binary or establish that it came from a particular release.
The entry digests compare catalog metadata; they are not hashes of the binary
or release archives and do not verify executable code. Archive SHA-256
sidecars are separate checks, and an unauthenticated checksum does not establish
publisher identity. For provenance, use an operator-signed manifest and a
public key obtained through a trusted channel, as described below.

### Tampering example

Suppose an attacker replaces the release binary with one that changes an
adapter's behavior and its compiled catalog, then supplies an unsigned manifest
exported from that replacement binary. Running the replacement binary's
`verify-manifest` with `--allow-unsigned` can succeed: its reported
`signature unsigned` and `drift none` mean only that the supplied manifest
matches the catalog the running binary reports under the comparison rules.
No signature was verified, and the result does not establish who supplied
either artifact or whether the executable contains the expected code. The
attacker can also alter executable behavior while retaining the original catalog,
so matching the original manifest would still not detect that change.

By contrast, if a separately trusted, unmodified binary checks the attacker's
changed catalog manifest, it reports drift and fails. A trusted operator's
signature can authenticate the manifest's canonical catalog material, but it
does not hash or authenticate the executable; assess the binary's provenance
independently even when the manifest is signed.

## What it is

`locus adapter registry export` emits a canonical JSON manifest of the
**built-in** adapter catalog compiled into the binary:

```json
{
  "schema": "locus-adapter-registry/v1",
  "locus_version": "0.5.0",
  "adapters": [
    {
      "id": "github",
      "name": "GitHub",
      "version": "0.5.0",
      "tools": ["github.check_repo", "github.scope", "github.whoami"],
      "digest": "sha256:…"
    }
  ]
}
```

This illustrates the unsigned release asset. An operator-signed manifest
also includes `signature` and `signed_by` fields.

- `version` is the Locus workspace (crate) version the adapter shipped in.
- `digest` is `sha256` over the entry's **canonical material** — id, name,
  status, synthetic flag, capabilities, frozen selectors, tools, and
  destructive tools. It detects changes to that catalog metadata when compared
  with a trusted catalog; it does not hash adapter implementation or detect
  behavior changes that leave the metadata unchanged.
- Adapters and tool lists are sorted; exporting twice from the same binary is
  byte-identical. `signature`/`signed_by` are excluded from the signed
  material, so signing does not perturb it.

## Trust model

| Party | Holds | Does |
|-------|-------|------|
| **Operator** (registry root) | ed25519 **private** seed | Can sign a manifest locally and distribute it out of band |
| **Consumer** (any Locus install) | ed25519 **public** key, pinned in the local trust store | Can verify an operator-signed manifest and compare it with the running binary |
| **CI** | nothing | Attaches the **unsigned** canonical manifest as a release asset (no signing secret is provisioned; see the `TODO(registry-signing)` block in `.github/workflows/release.yml`) |

Trust keys are the same store used by `locus adapter verify`
(merged, env wins on same id):

1. `$LOCUS_HOME/trust/adapter-keys.toml` (dir `0700`, file `0600`)
2. `LOCUS_ADAPTER_TRUST_KEYS=id:ed25519:<base64-pubkey>[,id:hmac-sha256:<64-hex>]`

`hmac-sha256` keys apply **only** to per-entry catalog verification. Release
manifests are ed25519-only: HMAC is symmetric, so every verifier's trust store
would hold the forging secret — `verify-manifest` reports such signatures as
`malformed` (fail closed).

## Commands

### Operator: optionally sign and distribute a manifest

```bash
# Signing key: base64 (or 64-hex) of the 32-byte ed25519 seed, in a local file.
# Locus never generates, prints, or logs private key material.
locus adapter registry export --sign --key ~/keys/locus-registry-root \
  --key-id root --out locus-adapters-v0.5.0.json

# Or via env (e.g. from a secrets manager; still never echoed):
export LOCUS_REGISTRY_SIGNING_KEY="<base64-seed>"
locus adapter registry export --sign --out locus-adapters-v0.5.0.json
```

`--sign` **refuses** to export when no key is available — there is no silent
fallback to unsigned output. Without `--sign` the export is the same canonical
JSON, just unsigned (what CI attaches to each GitHub release as
`locus-adapters-<tag>.json`).

Distribute the matching public key through a trusted channel; consumers pin it once:

```bash
locus adapter trust add --id root --ed25519-pub '<base64-pubkey>'
```

### Consumer: verify

```bash
locus adapter trust add --id root --ed25519-pub '<base64-pubkey>'   # once
locus adapter verify-manifest locus-adapters-v0.5.0.json [--json]
```

## What `verify-manifest` proves

For a signed manifest, both checks must pass (fail closed):

1. **Catalog signature** — the signature stored in the manifest verifies its
   canonical material against the key selected by `signed_by` in the local
   trust store. This authenticates that material to the holder of that key;
   the consumer must establish separately who controls the key and what they
   are authorized to sign. Unsigned, unknown-key, invalid, and malformed
   signatures all fail.
2. **No drift** — the running binary's built-in adapter set matches the
   manifest: same Locus version, adapter ids, names, adapter versions, tool
   lists, and per-adapter `sha256` digests. Comparison trims surrounding
   whitespace, matches adapter ids case-insensitively, and ignores tool order.
   Added, removed, or changed entries fail with a per-finding drift report.

Together these checks show that the catalog reported by the running binary
matches the catalog material signed by the holder of the locally trusted key.
They do not prove that the binary is authentic, that the signer built it, or
that the manifest belongs to a particular tag or archive. The signature also
does not establish freshness: a previously signed matching catalog can be
replayed. Establish the binary's provenance and the key's identity through
independent trusted channels.

`--allow-unsigned` relaxes **only** the unsigned case (a missing, null, empty,
or whitespace-only signature). Any nonempty signature that is malformed,
untrusted, or invalid still fails, even with the flag. Drift always fails.
An unsigned manifest may still contain a `signed_by` value; the reported name
is only a claim unless the signature status is `valid`.

## Non-goals

- Not a plugin loader — adapters remain in-tree
  (`crates/locus-core/src/adapters/`, registered in `adapter_for()`).
- Per-entry catalog signatures in `adapters/manifest.toml` are a separate,
  complementary surface (`locus adapter verify [--require-signed]`); see
  [adapter-sdk.md](./adapter-sdk.md).
