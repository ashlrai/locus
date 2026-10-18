# Post-quantum readiness

Status as of 2026-10-17. This note records what Locus does with cryptography today and what a post-quantum migration would touch. It makes no claim that Locus is quantum-safe or quantum-resistant, and it adds no new cryptography.

## Background (verified against NIST pages)
- NIST published FIPS 203 (ML-KEM), FIPS 204 (ML-DSA) and FIPS 205 (SLH-DSA) on August 13, 2024. Source: https://csrc.nist.gov/Projects/Post-Quantum-Cryptography/Post-Quantum-Cryptography-Standardization
- Shor's algorithm would break RSA and elliptic-curve schemes (RSA, ECDSA, ECDH, X25519, Ed25519) on a large enough quantum computer. Grover's algorithm gives a quadratic speedup on brute-force search, so symmetric keys are usually sized at 256 bits.
- Harvest-now-decrypt-later applies to confidentiality: ciphertext captured today, protected only by classical key exchange, could be read later. Signatures are different: they matter when they are verified, so the risk is forgery after a capable quantum computer exists.

## What Locus uses today
- **Ed25519 signatures (classical-only)** for adapter registry manifests: `crates/locus-core/src/adapter_registry.rs`, `adapter_trust.rs` and `marketplace.rs` use `ed25519-dalek`, with pinned public keys under `$LOCUS_HOME/trust/adapter-keys.toml`. This is the one place a quantum computer capable of running Shor's algorithm would allow forgery. Signatures carry a scheme prefix (`ed25519:`), so a second scheme can be added without changing the manifest layout; no post-quantum scheme exists in Locus today.
- **HMAC-SHA256** is accepted as a backward-compatible registry signature (`hmac-sha256:`), a shared-secret scheme.
- **ChaCha20-Poly1305** (a 256-bit-key AEAD) encrypts exported graph files (`crates/locus-core/src/graph.rs`, format `LOCUSGRAPH1`) under a key derived from a passphrase with Argon2id.
- **SHA-256** digests identify adapter sets and release assets (`.sha256` sidecars).
- Approval receipts carry a `signature_algorithm` field (`crates/locus-core/src/approval.rs`), which is the hook for algorithm agility there.

## The check
`scripts/check-crypto.mjs` scans source files (not tests, docs or vendored directories) for classical asymmetric primitives: RSA, ECDSA/ECDH, Ed25519/X25519 and Diffie-Hellman key generation. It prints an inventory and exits 1 when a file that is not in its `ALLOW` list uses one, so a new classical-only use needs a deliberate decision with a written reason. It does not judge hashes, HMAC or symmetric ciphers, and it is a regex scan, not a parser: it can miss indirect use through a dependency. Run it with `node scripts/check-crypto.mjs`; its tests run with `node --test scripts/check-crypto.test.mjs`.

## Library note
No post-quantum library is used here, because there is no concrete use that needs one. If one is added later, `@noble/post-quantum` 0.7.1 (MIT) offers ML-KEM, ML-DSA and SLH-DSA in TypeScript, but its README states it is not independently audited and not constant-time, so it should not be used for secret-key signing on shared hardware. RustCrypto `ml-kem` and `ml-dsa` carry the same "never independently audited" statement. Any adoption should say so in its docs. Source notes: Phantom's quantum research, /workspace/phantom-work/quantum-research.md.

## If a migration is needed
1. Keep the scheme prefix mandatory so verifiers reject unknown schemes explicitly.
2. When a post-quantum or hybrid signature (ML-DSA or SLH-DSA, FIPS 204/205) is chosen, add it as an additional accepted scheme next to `ed25519:` and let a manifest carry both signatures during a transition.
3. Use a library whose audit status is stated in the docs; the Rust candidates named above are unaudited.
4. The graph export is symmetric and passphrase-based, so it needs no change for Shor; the passphrase key derivation (Argon2id) is the part to review.
