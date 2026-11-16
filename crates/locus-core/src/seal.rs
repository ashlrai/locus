//! Session seals — HMAC over session identity so pins cannot be forged.

use crate::error::{LocusError, Result};
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::Sha256;
use zeroize::{Zeroize, ZeroizeOnDrop};

type HmacSha256 = Hmac<Sha256>;

/// 256-bit daemon sealing key. Zeroized on drop.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct SealKey([u8; 32]);

impl SealKey {
    pub fn generate() -> Self {
        let mut buf = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut buf);
        Self(buf)
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn from_hex(s: &str) -> Result<Self> {
        let bytes = hex::decode(s.trim()).map_err(|e| LocusError::msg(e.to_string()))?;
        if bytes.len() != 32 {
            return Err(LocusError::msg("seal key must be 32 bytes (64 hex chars)"));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Ok(Self(arr))
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    pub fn seal(&self, material: &str) -> String {
        let mut mac = HmacSha256::new_from_slice(&self.0).expect("HMAC accepts any key length");
        mac.update(material.as_bytes());
        let result = mac.finalize().into_bytes();
        format!("hmac-sha256:{}", hex::encode(result))
    }

    pub fn verify(&self, material: &str, seal: &str) -> bool {
        let expected = self.seal(material);
        // Constant-time-ish compare via hmac crate's approach: equal lengths then byte xor
        if expected.len() != seal.len() {
            return false;
        }
        let mut diff = 0u8;
        for (a, b) in expected.bytes().zip(seal.bytes()) {
            diff |= a ^ b;
        }
        diff == 0
    }
}

/// Canonical seal material for a session.
pub fn seal_material(
    session_id: &str,
    binding_id: &str,
    pinned_at: &str,
    expires_at: &str,
) -> String {
    format!("{session_id}|{binding_id}|{pinned_at}|{expires_at}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_roundtrip() {
        let key = SealKey::generate();
        let m = seal_material("ses_1", "bnd_a", "t0", "t1");
        let s = key.seal(&m);
        assert!(key.verify(&m, &s));
        assert!(!key.verify(&m, "hmac-sha256:deadbeef"));
        assert!(!key.verify("tampered", &s));
    }

    #[test]
    fn hex_roundtrip() {
        let key = SealKey::generate();
        let h = key.to_hex();
        let key2 = SealKey::from_hex(&h).unwrap();
        let m = "hello";
        assert_eq!(key.seal(m), key2.seal(m));
    }

    #[test]
    fn seal_matches_known_hmac_and_rejects_changed_identity() {
        let key = SealKey::from_bytes([0x0b; 32]);
        assert_eq!(
            key.seal("Hi There"),
            "hmac-sha256:198a607eb44bfbc69903a0f1cf2bbdc5ba0aa3f3d9ae3c1c7a3b1696a0b68cf7"
        );
        let material = seal_material("session", "binding", "start", "end");
        let seal = key.seal(&material);
        for changed in [
            seal_material("other", "binding", "start", "end"),
            seal_material("session", "other", "start", "end"),
            seal_material("session", "binding", "other", "end"),
            seal_material("session", "binding", "start", "other"),
        ] {
            assert!(!key.verify(&changed, &seal));
        }
        assert!(!SealKey::from_bytes([0x0c; 32]).verify(&material, &seal));
        let mut altered = seal.into_bytes();
        *altered.last_mut().unwrap() ^= 1;
        assert!(!key.verify(&material, std::str::from_utf8(&altered).unwrap()));
    }

    #[test]
    fn key_hex_validation_rejects_malformed_and_wrong_lengths() {
        for input in [
            "",
            "00",
            &"00".repeat(31),
            &"00".repeat(33),
            &"zz".repeat(32),
        ] {
            assert!(SealKey::from_hex(input).is_err());
        }
        let hex = "ab".repeat(32);
        assert_eq!(
            SealKey::from_hex(&format!("  {hex}\n")).unwrap().to_hex(),
            hex
        );
    }
}
