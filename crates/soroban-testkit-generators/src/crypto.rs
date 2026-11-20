//! Cryptographic signature and key validation proptest strategies.
//!
//! Provides strategies for generating valid Ed25519 and Secp256k1 public keys,
//! message digests, and signatures verifiable by `soroban_sdk::crypto`, as well
//! as malleated and corrupted signature variants for robustness testing.

use proptest::prelude::*;

use crate::tuples::byte_array_32;

/// A generated Ed25519 keypair and valid signature over a message digest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ed25519SignaturePair {
    /// 32-byte Ed25519 public key.
    pub public_key: [u8; 32],
    /// 32-byte message or pre-hashed digest.
    pub message_digest: [u8; 32],
    /// 64-byte Ed25519 signature.
    pub signature: [u8; 64],
}

impl Ed25519SignaturePair {
    /// Public key bytes.
    pub fn public_key(&self) -> &[u8; 32] {
        &self.public_key
    }

    /// Message digest bytes.
    pub fn message_digest(&self) -> &[u8; 32] {
        &self.message_digest
    }

    /// Signature bytes.
    pub fn signature(&self) -> &[u8; 64] {
        &self.signature
    }

    /// Destructure into a tuple `(public_key, message_digest, signature)`.
    pub fn into_parts(self) -> ([u8; 32], [u8; 32], [u8; 64]) {
        (self.public_key, self.message_digest, self.signature)
    }
}

impl From<Ed25519SignaturePair> for ([u8; 32], [u8; 32], [u8; 64]) {
    fn from(p: Ed25519SignaturePair) -> Self {
        (p.public_key, p.message_digest, p.signature)
    }
}

/// A generated Secp256k1 ECDSA public key, message hash, signature, and recovery ID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Secp256k1SignaturePair {
    /// SEC-1 uncompressed 65-byte public key (prefix 0x04).
    pub public_key: [u8; 65],
    /// 32-byte message hash.
    pub message_hash: [u8; 32],
    /// 64-byte compact ECDSA signature `(r, s)`.
    pub signature: [u8; 64],
    /// Recovery ID (0, 1, 2, or 3).
    pub recovery_id: u32,
}

impl Secp256k1SignaturePair {
    /// Uncompressed 65-byte public key.
    pub fn public_key(&self) -> &[u8; 65] {
        &self.public_key
    }

    /// 32-byte message hash.
    pub fn message_hash(&self) -> &[u8; 32] {
        &self.message_hash
    }

    /// 64-byte signature.
    pub fn signature(&self) -> &[u8; 64] {
        &self.signature
    }

    /// Recovery ID.
    pub fn recovery_id(&self) -> u32 {
        self.recovery_id
    }

    /// Destructure into a tuple `(public_key, message_hash, signature, recovery_id)`.
    pub fn into_parts(self) -> ([u8; 65], [u8; 32], [u8; 64], u32) {
        (
            self.public_key,
            self.message_hash,
            self.signature,
            self.recovery_id,
        )
    }
}

impl From<Secp256k1SignaturePair> for ([u8; 65], [u8; 32], [u8; 64]) {
    fn from(p: Secp256k1SignaturePair) -> Self {
        (p.public_key, p.message_hash, p.signature)
    }
}

/// Generates a valid Ed25519 public key, 32-byte message digest, and valid signature.
///
/// Shrinks towards simpler message hashes (closer to all zeros) and smaller seed values.
pub fn ed25519_signature_pair() -> impl Strategy<Value = Ed25519SignaturePair> {
    (byte_array_32(), byte_array_32()).prop_map(|(seed, msg)| {
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&seed);
        let verifying_key = signing_key.verifying_key();
        let public_key = verifying_key.to_bytes();
        let sig = ed25519_dalek::Signer::sign(&signing_key, &msg);
        let signature = sig.to_bytes();
        Ed25519SignaturePair {
            public_key,
            message_digest: msg,
            signature,
        }
    })
}

/// Generates a valid Secp256k1 public key, 32-byte message hash, 64-byte compact signature, and recovery ID.
///
/// Shrinks towards simpler message hashes while maintaining cryptographic validity.
pub fn secp256k1_signature_pair() -> impl Strategy<Value = Secp256k1SignaturePair> {
    use sha2::Digest as _;
    (byte_array_32(), byte_array_32()).prop_filter_map(
        "valid non-zero scalar key and signature",
        |(seed, msg)| {
            let signing_key = k256::ecdsa::SigningKey::from_slice(&seed).ok()?;
            let verifying_key = signing_key.verifying_key();
            let pubkey_point = verifying_key.to_encoded_point(false);
            let pubkey_bytes: [u8; 65] = pubkey_point.as_bytes().try_into().ok()?;
            let hash: [u8; 32] = sha2::Sha256::digest(msg).into();
            let (signature, recovery_id) = signing_key.sign_prehash_recoverable(&hash).ok()?;
            let signature_bytes: [u8; 64] = signature.to_bytes().into();
            Some(Secp256k1SignaturePair {
                public_key: pubkey_bytes,
                message_hash: msg,
                signature: signature_bytes,
                recovery_id: recovery_id.to_byte() as u32,
            })
        },
    )
}

/// Mutates a valid 64-byte signature to test that verification rejects corrupted payloads.
///
/// Shrinks towards the simplest single-bit perturbation.
pub fn malleated_signature(valid_sig: [u8; 64]) -> impl Strategy<Value = [u8; 64]> {
    prop_oneof![
        3 => (0usize..64, 1u8..=255).prop_map(move |(idx, mask)| {
            let mut sig = valid_sig;
            sig[idx] ^= mask;
            sig
        }),
        1 => (0usize..32).prop_map(move |idx| {
            let mut sig = valid_sig;
            sig[32 + idx] ^= 0x80;
            sig
        }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{Bytes, BytesN, Env};
    use std::panic::catch_unwind;

    proptest! {
        #[test]
        fn test_ed25519_signature_verifies(pair in ed25519_signature_pair()) {
            let env = Env::default();
            let pk = BytesN::from_array(&env, pair.public_key());
            let msg = Bytes::from_slice(&env, pair.message_digest());
            let sig = BytesN::from_array(&env, pair.signature());

            // Valid signature verification succeeds without panic
            env.crypto().ed25519_verify(&pk, &msg, &sig);
        }

        #[test]
        fn test_ed25519_malleated_signature_rejected(
            pair in ed25519_signature_pair(),
            corrupted_sig in crate::tuples::byte_array_64(),
        ) {
            let env = Env::default();
            let pk = BytesN::from_array(&env, pair.public_key());
            let msg = Bytes::from_slice(&env, pair.message_digest());

            // Ensure corrupted signature is different from valid signature
            prop_assume!(&corrupted_sig != pair.signature());
            let bad_sig = BytesN::from_array(&env, &corrupted_sig);

            let result = catch_unwind(std::panic::AssertUnwindSafe(|| {
                env.crypto().ed25519_verify(&pk, &msg, &bad_sig);
            }));
            prop_assert!(result.is_err(), "malleated signature should fail verification");
        }

        #[test]
        fn test_secp256k1_signature_recovers_pubkey(pair in secp256k1_signature_pair()) {
            let env = Env::default();
            let hash_bytes = Bytes::from_slice(&env, pair.message_hash());
            let hash = env.crypto().sha256(&hash_bytes);
            let sig = BytesN::from_array(&env, pair.signature());

            // Recover public key and verify exact match
            let recovered = env.crypto().secp256k1_recover(&hash, &sig, pair.recovery_id());
            assert_eq!(recovered.to_array(), *pair.public_key());
        }

        #[test]
        fn test_secp256k1_malleated_signature_fails_or_mismatches(
            pair in secp256k1_signature_pair(),
            corrupted_sig in crate::tuples::byte_array_64(),
        ) {
            let env = Env::default();
            let hash_bytes = Bytes::from_slice(&env, pair.message_hash());
            let hash = env.crypto().sha256(&hash_bytes);
            let expected_pubkey = *pair.public_key();

            prop_assume!(&corrupted_sig != pair.signature());
            let bad_sig = BytesN::from_array(&env, &corrupted_sig);

            let result = catch_unwind(std::panic::AssertUnwindSafe(|| {
                env.crypto().secp256k1_recover(&hash, &bad_sig, pair.recovery_id())
            }));
            if let Ok(recovered) = result {
                prop_assert_ne!(recovered.to_array(), expected_pubkey);
            }
        }

        #[test]
        fn test_malleated_signature_always_mutates(
            sig in crate::tuples::byte_array_64(),
        ) {
            let mut runner = proptest::test_runner::TestRunner::default();
            runner.run(&malleated_signature(sig), |mutated| {
                prop_assert_ne!(mutated, sig);
                Ok(())
            }).unwrap();
        }
    }
}
