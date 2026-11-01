# Release manifest consumer checklist

Use this checklist before relying on the adapter registry release manifest
attached to a Locus release. It is derived from
[registry-trust.md](./registry-trust.md) and summarizes what the artifact is,
what it is for, and what checking it can and cannot establish today. For the
command walkthrough see
[registry-manifest-verification.md](./registry-manifest-verification.md); for
common questions see the [registry trust FAQ](./registry-trust-faq.md).

Check each item as you confirm it. An unchecked item is an open question
about your own review, not a defect in the artifact.

## 1. Identify the artifact

- [ ] **File:** `locus-adapters-<tag>.json`, attached by the release workflow
  to the tag-push release for `<tag>`. The filename names a tag, but the
  manifest does not bind that tag, a commit, a build process, or any release
  archive.
- [ ] **Signature status: unsigned.** Release CI exports the manifest with
  `locus adapter registry export` **without** `--sign`. There is no
  configured CI signing setup and no published registry root for this
  artifact; CI signing is future work, not a current release guarantee.
- [ ] **Intended use:** a canonical JSON snapshot of the built-in adapter
  catalog — adapter ids, names, versions, tool lists, and per-entry digests —
  for a *local drift check* against the catalog compiled into the Locus
  binary you run. It is catalog metadata, not a signed provenance statement,
  and it does not load or install adapters.

## 2. Obtain artifacts

- [ ] Get the manifest and a Locus CLI **for the same release version** — the
  check compares the file with the catalog embedded in the binary that runs
  it, not with the release it was named after.
- [ ] Assess the source of each artifact independently. The check cannot
  vouch for where either one came from.

## 3. Run the check

- [ ] Run the drift comparison, substituting the real asset name:

  ```bash
  locus adapter verify-manifest locus-adapters-v0.6.2.json --allow-unsigned
  ```

- [ ] Replace `v0.6.2` with the actual release tag; quote the filename if it
  contains shell-special characters. The command reads the local file and
  does not download either artifact.
- [ ] Expected pass output: `signature unsigned`, `drift none`, exit 0. Add
  `--json` for a machine-readable report.
- [ ] Without `--allow-unsigned`, the unsigned asset fails even when the
  catalog matches. That is the fail-closed default, not a malfunction.
- [ ] Treat any nonzero exit as needing investigation: the parser rejects
  invalid JSON, an unsupported schema, empty or duplicate adapter ids, and
  ambiguous canonical-material separators; the drift check reports added,
  missing, or changed adapter entries.
- [ ] The comparison checks the Locus version and adapter ids, names,
  versions, tool lists, and entry digests. It trims surrounding whitespace,
  matches ids case-insensitively, and ignores tool order. Entry descriptions
  and per-entry signatures and signers are not compared.
- [ ] `--allow-unsigned` excuses **only** a missing or empty signature. A
  nonempty malformed, untrusted, or invalid signature still fails, and drift
  always fails. A `signed_by` value on an unsigned manifest is a claim, not
  a verification result.

## 4. What a pass does not establish (current limits)

- [ ] **No provenance.** A pass is catalog agreement under the documented
  comparison rules only. An unsigned manifest does not authenticate its
  publisher, origin, or integrity against tampering.
- [ ] **No binary authentication.** Agreement does not prove the running
  binary is authentic or came from a particular release: a replaced binary
  and a manifest exported from it can still agree. See the
  [tampering example](./registry-trust.md#tampering-example).
- [ ] **Metadata digests only.** Entry digests cover canonical catalog
  metadata (id, name, status, synthetic flag, capabilities, frozen selectors,
  tools, destructive tools), not adapter code. Changed behavior with
  unchanged catalog metadata is invisible to the check.
- [ ] **No artifact coverage.** The manifest does not hash release archives,
  the `locus`/`locus-mcp` executables inside them, `.sha256` sidecars,
  packaged docs, or npm packages.
- [ ] **Sidecars are a separate check.** Archive `.sha256` sidecars compare
  archive bytes with the sidecar digest; an attacker who replaces both files
  passes it. See
  [check an archive separately](./registry-manifest-verification.md#check-an-archive-separately).
- [ ] **No freshness.** Nothing binds the manifest to a point in time, so a
  previously matching catalog can be replayed.

## 5. Optional: an operator-signed manifest

Separate from release CI, an operator may sign a manifest with Ed25519 and
distribute the public key through a trusted channel. If you consume such a
manifest:

- [ ] Pin the public key once
  (`locus adapter trust add --id <key-id> --ed25519-pub <base64-pubkey>`),
  then verify **without** `--allow-unsigned`: a pass requires both a valid
  signature and no drift.
- [ ] Establish who controls the key and that they are authorized to sign.
  The signature authenticates the canonical catalog material to the key
  holder; it does not identify the holder by itself.
- [ ] The same limits still apply: a valid signature does not authenticate
  the binary, archive, tag, commit, or build, and does not establish
  freshness.
