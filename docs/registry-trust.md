# Adapter registry release manifests (signed)

> Companion to the [Adapter SDK](./adapter-sdk.md) "Signed registry roadmap".
> This page covers the **release manifest**: a per-release, signable snapshot of
> the built-in adapter set.

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
  ],
  "signature": "ed25519:…",
  "signed_by": "root"
}
```

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
| **Operator** (registry root) | ed25519 **private** seed (offline, never in git/CI) | Signs the manifest at release time, distributes `locus-adapters-<tag>.json` |
| **Consumer** (any Locus install) | ed25519 **public** key, pinned in the local trust store | Verifies signature + asserts the running binary matches the manifest |
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

### Operator: sign at release time

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

Publish the matching public key once:

```bash
locus adapter registry export --sign --key … # signed manifest to distribute
# consumers pin:
locus adapter trust add --id root --ed25519-pub '<base64-pubkey>'
```

### Consumer: verify

```bash
locus adapter trust add --id root --ed25519-pub '<base64-pubkey>'   # once
locus adapter verify-manifest locus-adapters-v0.5.0.json [--json]
```

## What `verify-manifest` proves

Both checks must pass (fail closed):

1. **Provenance** — the manifest's detached signature verifies against a key
   pinned in the local trust store, and `signed_by` names that key. Unsigned,
   unknown-key, invalid, and malformed signatures all fail.
2. **No drift** — the running binary's built-in adapter set matches the
   manifest **exactly**: same Locus version, same adapter ids, names, tool
   lists, and per-adapter `sha256` digests. An adapter added, removed, or
   changed on either side fails with a per-finding drift report.

Together: *"the adapter surface this binary exposes is exactly the set the
registry root signed for this release."*

`--allow-unsigned` relaxes **only** the missing-signature case (useful for
drift-checking the unsigned CI asset). A signature that is present but
untrusted or invalid still fails, even with the flag.

## Non-goals

- Not a plugin loader — adapters remain in-tree
  (`crates/locus-core/src/adapters/`, registered in `adapter_for()`).
- Per-entry catalog signatures in `adapters/manifest.toml` are a separate,
  complementary surface (`locus adapter verify [--require-signed]`); see
  [adapter-sdk.md](./adapter-sdk.md).
