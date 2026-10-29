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

To check the current release asset for drift, use the corresponding Locus
binary and explicitly opt in to unsigned verification:

```bash
locus adapter verify-manifest locus-adapters-v0.5.0.json --allow-unsigned
```

A successful result compares the supplied manifest with the catalog embedded
in that binary. It authenticates neither the manifest nor the binary; obtain and
assess both through a trusted distribution channel.

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
  destructive tools — so it detects any behavioral catalog change, not just
  tool renames.
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

1. **Provenance** — the signature stored in the manifest verifies its canonical
   material against a key pinned in the local trust store, and `signed_by`
   names that key. Unsigned, unknown-key, invalid, and malformed signatures
   all fail.
2. **No drift** — the running binary's built-in adapter set matches the
   manifest: same Locus version, adapter ids, names, adapter versions, tool
   lists, and per-adapter `sha256` digests. Comparison trims surrounding
   whitespace, matches adapter ids case-insensitively, and ignores tool order.
   Added, removed, or changed entries fail with a per-finding drift report.

Together: *"the adapter catalog in this binary matches the set the
registry root signed for this release."*

`--allow-unsigned` relaxes **only** the unsigned case (a missing, null, empty,
or whitespace-only signature). Any nonempty signature that is malformed,
untrusted, or invalid still fails, even with the flag. Drift always fails.

## Non-goals

- Not a plugin loader — adapters remain in-tree
  (`crates/locus-core/src/adapters/`, registered in `adapter_for()`).
- Per-entry catalog signatures in `adapters/manifest.toml` are a separate,
  complementary surface (`locus adapter verify [--require-signed]`); see
  [adapter-sdk.md](./adapter-sdk.md).
