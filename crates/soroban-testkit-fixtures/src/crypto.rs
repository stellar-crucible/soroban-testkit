//! Cryptographic signing keypairs and authorization fixtures for Soroban testing.
//!
//! While `env.mock_all_auths()` is useful for initial unit testing, contracts handling
//! off-chain signed meta-transactions, delegated account credentials, or multi-sig
//! policies require testing with authentic Ed25519 signatures and real cryptographic keypairs.
//!
//! This module provides [`SignerKeypair`], wrapping 32-byte Ed25519 keys, producing
//! host-compatible [`Address`] values, signing arbitrary payloads and authorization structures,
//! and helpers for simulating replay attacks and signature malleability.

use ed25519_dalek::{Signer as _, SigningKey, VerifyingKey};
use rand_core::OsRng;
use soroban_sdk::testutils::AuthorizedInvocation;
use soroban_sdk::xdr::{AccountId, PublicKey, ScAddress, Uint256};
use soroban_sdk::{Address, Bytes, BytesN, Env, TryFromVal};

/// An Ed25519 signing keypair that generates host-compatible [`Address`] values
/// and cryptographically authentic signatures verifiable by `soroban_sdk::crypto`.
#[derive(Clone)]
pub struct SignerKeypair {
    signing_key: SigningKey,
    verifying_key: VerifyingKey,
    address: Address,
}

impl std::fmt::Debug for SignerKeypair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignerKeypair")
            .field("public_key", &format!("{:02x?}", self.public_key()))
            .field("address", &self.address)
            .finish()
    }
}

impl SignerKeypair {
    /// Generates a new random Ed25519 keypair for the given environment.
    pub fn random(env: &Env) -> Self {
        let signing_key = SigningKey::generate(&mut OsRng);
        Self::from_signing_key(env, signing_key)
    }

    /// Creates a keypair from a 32-byte secret seed / private key.
    pub fn from_seed(env: &Env, seed: &[u8; 32]) -> Self {
        let signing_key = SigningKey::from_bytes(seed);
        Self::from_signing_key(env, signing_key)
    }

    /// Alias for [`SignerKeypair::from_seed`].
    pub fn from_secret_key_bytes(env: &Env, secret_bytes: &[u8; 32]) -> Self {
        Self::from_seed(env, secret_bytes)
    }

    fn from_signing_key(env: &Env, signing_key: SigningKey) -> Self {
        let verifying_key = signing_key.verifying_key();
        let sc_address = ScAddress::Account(AccountId(PublicKey::PublicKeyTypeEd25519(Uint256(
            verifying_key.to_bytes(),
        ))));
        let address = Address::try_from_val(env, &sc_address)
            .expect("32-byte Ed25519 public key converts to valid Account Address");

        Self {
            signing_key,
            verifying_key,
            address,
        }
    }

    /// The host-compatible account [`Address`] derived from this Ed25519 public key.
    pub fn address(&self) -> &Address {
        &self.address
    }

    /// The raw 32-byte Ed25519 public key.
    pub fn public_key(&self) -> [u8; 32] {
        self.verifying_key.to_bytes()
    }

    /// The 32-byte Ed25519 public key wrapped in [`BytesN<32>`] for Soroban crypto calls.
    pub fn public_key_bytes(&self, env: &Env) -> BytesN<32> {
        BytesN::from_array(env, &self.public_key())
    }

    /// The raw 32-byte Ed25519 secret key.
    pub fn secret_key_bytes(&self) -> [u8; 32] {
        self.signing_key.to_bytes()
    }

    /// The inner dalek [`VerifyingKey`].
    pub fn verifying_key(&self) -> &VerifyingKey {
        &self.verifying_key
    }

    /// Signs a Soroban [`Bytes`] message and returns a 64-byte signature [`BytesN<64>`].
    pub fn sign(&self, message: &Bytes) -> BytesN<64> {
        let mut raw_bytes = vec![0u8; message.len() as usize];
        message.copy_into_slice(&mut raw_bytes);
        let sig = self.signing_key.sign(&raw_bytes);
        BytesN::from_array(message.env(), &sig.to_bytes())
    }

    /// Signs an arbitrary byte slice and returns a 64-byte signature [`BytesN<64>`].
    pub fn sign_slice(&self, env: &Env, message: &[u8]) -> BytesN<64> {
        let sig = self.signing_key.sign(message);
        BytesN::from_array(env, &sig.to_bytes())
    }

    /// Signs a 32-byte message digest/hash and returns a 64-byte signature [`BytesN<64>`].
    pub fn sign_hash(&self, env: &Env, hash: &[u8; 32]) -> BytesN<64> {
        self.sign_slice(env, hash)
    }

    /// Signs an arbitrary byte slice and returns the raw 64-byte signature array.
    pub fn sign_raw(&self, message: &[u8]) -> [u8; 64] {
        self.signing_key.sign(message).to_bytes()
    }

    /// Verify this keypair's signature over `message` using Dalek verifier (without panicking).
    pub fn verify_signature(&self, message: &[u8], signature_bytes: &[u8; 64]) -> bool {
        use ed25519_dalek::Verifier as _;
        if let Ok(sig) = ed25519_dalek::Signature::from_slice(signature_bytes) {
            self.verifying_key.verify(message, &sig).is_ok()
        } else {
            false
        }
    }
}

/// Mutates a 64-byte signature to test that verification rejects corrupted payloads.
///
/// Flips a bit in the signature, producing an invalid cryptographic proof.
pub fn malleate_signature(env: &Env, signature: &BytesN<64>) -> BytesN<64> {
    let mut bytes = signature.to_array();
    bytes[0] ^= 0x01;
    BytesN::from_array(env, &bytes)
}

/// Mutates a 64-byte raw signature array to test signature malleability.
pub fn malleate_signature_bytes(signature: &[u8; 64]) -> [u8; 64] {
    let mut bytes = *signature;
    bytes[0] ^= 0x01;
    bytes
}

/// Mutates a [`Bytes`] message payload by modifying the first byte (or adding one if empty).
pub fn mutate_message(message: &Bytes) -> Bytes {
    let env = message.env();
    let mut raw = vec![0u8; message.len() as usize];
    message.copy_into_slice(&mut raw);
    if raw.is_empty() {
        raw.push(0xff);
    } else {
        raw[0] ^= 0x01;
    }
    Bytes::from_slice(env, &raw)
}

/// A structured authorization envelope combining an [`AuthorizedInvocation`]
/// with cryptographic proof of signer authorization.
#[derive(Clone, Debug, PartialEq)]
pub struct SignedAuth {
    /// The Soroban authorized invocation structure.
    pub invocation: AuthorizedInvocation,
    /// The signer address.
    pub signer: Address,
    /// Cryptographic Ed25519 signature over the serialized authorization payload.
    pub signature: BytesN<64>,
    /// The message bytes that were signed.
    pub message: Bytes,
    /// Nonce used for replay protection.
    pub nonce: u64,
}

impl SignedAuth {
    /// Reference to the underlying [`AuthorizedInvocation`].
    pub fn invocation(&self) -> &AuthorizedInvocation {
        &self.invocation
    }

    /// Consumes the wrapper and returns the [`AuthorizedInvocation`].
    pub fn into_invocation(self) -> AuthorizedInvocation {
        self.invocation
    }

    /// Simulates a replay attack with an incremented or altered nonce while keeping
    /// the original signature, which should fail replay verification.
    pub fn simulate_replay_attack(&self, replay_nonce: u64) -> Self {
        Self {
            invocation: self.invocation.clone(),
            signer: self.signer.clone(),
            signature: self.signature.clone(),
            message: self.message.clone(),
            nonce: replay_nonce,
        }
    }

    /// Produces a copy with a malleated/corrupted signature to test signature rejection.
    pub fn with_malleated_signature(&self) -> Self {
        let env = self.message.env();
        Self {
            invocation: self.invocation.clone(),
            signer: self.signer.clone(),
            signature: malleate_signature(env, &self.signature),
            message: self.message.clone(),
            nonce: self.nonce,
        }
    }

    /// Produces a copy with an altered message payload to test digest mismatch.
    pub fn with_altered_payload(&self) -> Self {
        Self {
            invocation: self.invocation.clone(),
            signer: self.signer.clone(),
            signature: self.signature.clone(),
            message: mutate_message(&self.message),
            nonce: self.nonce,
        }
    }

    /// Produces a copy attributed to a different signer address to test wrong public key.
    pub fn with_wrong_signer(&self, wrong_signer: &Address) -> Self {
        Self {
            invocation: self.invocation.clone(),
            signer: wrong_signer.clone(),
            signature: self.signature.clone(),
            message: self.message.clone(),
            nonce: self.nonce,
        }
    }
}

impl std::ops::Deref for SignedAuth {
    type Target = AuthorizedInvocation;

    fn deref(&self) -> &Self::Target {
        &self.invocation
    }
}

/// Constructs a deterministic message payload from contract, function, args and nonce.
pub fn construct_auth_message(
    env: &Env,
    contract: &Address,
    function: &soroban_sdk::Symbol,
    args: &soroban_sdk::Vec<soroban_sdk::Val>,
    nonce: u64,
) -> Bytes {
    let mut payload = Vec::new();
    // Prefix domain separator
    payload.extend_from_slice(b"soroban-auth-v1:");
    // Contract address string representation
    let contract_str = contract.to_string();
    let mut buf = vec![0u8; contract_str.len() as usize];
    contract_str.copy_into_slice(&mut buf);
    payload.extend_from_slice(&buf);
    payload.push(b':');
    // Function name
    let fn_str = function.to_string();
    payload.extend_from_slice(fn_str.as_bytes());
    payload.push(b':');
    // Nonce
    payload.extend_from_slice(&nonce.to_be_bytes());
    // Args count
    payload.extend_from_slice(&args.len().to_be_bytes());

    Bytes::from_slice(env, &payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::{Address as _, AuthorizedFunction};
    use soroban_sdk::{symbol_short, vec, Env};

    #[test]
    fn test_signer_keypair_generation_and_address() {
        let env = Env::default();
        let signer = SignerKeypair::random(&env);

        let pk_bytes = signer.public_key();
        assert_eq!(pk_bytes.len(), 32);

        // Address is valid and can be checked
        let addr = signer.address();
        assert_eq!(addr.clone(), *signer.address());
    }

    #[test]
    fn test_signature_verifies_with_soroban_crypto() {
        let env = Env::default();
        let signer = SignerKeypair::random(&env);
        let message = Bytes::from_slice(&env, b"Authorize transfer 500");
        let signature = signer.sign(&message);
        let pk = signer.public_key_bytes(&env);

        // soroban_sdk::crypto verification succeeds without panic
        env.crypto().ed25519_verify(&pk, &message, &signature);
    }

    #[test]
    fn test_malleated_signature_rejected() {
        let env = Env::default();
        let signer = SignerKeypair::random(&env);
        let message = Bytes::from_slice(&env, b"Authorize transfer 500");
        let valid_signature = signer.sign(&message);
        let bad_signature = malleate_signature(&env, &valid_signature);

        let pk = signer.public_key_bytes(&env);

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            env.crypto().ed25519_verify(&pk, &message, &bad_signature);
        }));
        assert!(
            result.is_err(),
            "malleated signature must fail verification"
        );
    }

    #[test]
    fn test_mutated_message_rejected() {
        let env = Env::default();
        let signer = SignerKeypair::random(&env);
        let message = Bytes::from_slice(&env, b"Authorize transfer 500");
        let signature = signer.sign(&message);
        let mutated_message = mutate_message(&message);

        let pk = signer.public_key_bytes(&env);

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            env.crypto()
                .ed25519_verify(&pk, &mutated_message, &signature);
        }));
        assert!(result.is_err(), "mutated message must fail verification");
    }

    #[test]
    fn test_wrong_public_key_rejected() {
        let env = Env::default();
        let signer1 = SignerKeypair::random(&env);
        let signer2 = SignerKeypair::random(&env);
        let message = Bytes::from_slice(&env, b"Authorize transfer 500");
        let signature = signer1.sign(&message);

        let wrong_pk = signer2.public_key_bytes(&env);

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            env.crypto().ed25519_verify(&wrong_pk, &message, &signature);
        }));
        assert!(result.is_err(), "wrong public key must fail verification");
    }

    #[test]
    fn test_signed_auth_helpers() {
        let env = Env::default();
        let signer = SignerKeypair::random(&env);
        let contract = Address::generate(&env);
        let function = symbol_short!("transfer");
        let args = vec![&env];

        let msg = construct_auth_message(&env, &contract, &function, &args, 42);
        let sig = signer.sign(&msg);

        let invocation = AuthorizedInvocation {
            function: AuthorizedFunction::Contract((contract.clone(), function, args)),
            sub_invocations: Vec::new(),
        };

        let auth = SignedAuth {
            invocation,
            signer: signer.address().clone(),
            signature: sig,
            message: msg,
            nonce: 42,
        };

        // Replay attack simulation changes nonce
        let replay = auth.simulate_replay_attack(43);
        assert_eq!(replay.nonce, 43);
        assert_eq!(replay.signature, auth.signature);

        // Malleated signature
        let malleated = auth.with_malleated_signature();
        assert_ne!(malleated.signature, auth.signature);

        // Altered payload
        let altered = auth.with_altered_payload();
        assert_ne!(altered.message, auth.message);

        // Wrong signer
        let other_signer = SignerKeypair::random(&env);
        let wrong = auth.with_wrong_signer(other_signer.address());
        assert_eq!(wrong.signer, *other_signer.address());
    }

    #[test]
    fn test_from_seed_reproducible() {
        let env = Env::default();
        let seed = [7u8; 32];
        let signer1 = SignerKeypair::from_seed(&env, &seed);
        let signer2 = SignerKeypair::from_seed(&env, &seed);

        assert_eq!(signer1.public_key(), signer2.public_key());
        assert_eq!(signer1.address(), signer2.address());

        let msg = Bytes::from_slice(&env, b"Deterministic signature");
        assert_eq!(signer1.sign(&msg), signer2.sign(&msg));
    }
}
