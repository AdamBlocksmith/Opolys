//! ed25519 signing and verification for the Opolys blockchain.
//!
//! Wallet signing on Opolys uses ed25519 (Curve25519 in EdDSA mode). Keys are
//! derived from BIP39 24-word mnemonics via SLIP-0010 ed25519 derivation, and
//! the resulting 32-byte public keys are hashed with Blake3-256 to produce
//! on-chain `ObjectId` addresses. This module provides verification only;
//! key generation and signing happen at the wallet layer.

use ed25519_dalek::{Signature as DalekSignature, Verifier, VerifyingKey};
use opolys_core::ObjectId;
use subtle::{Choice, ConstantTimeEq};

/// Derive an on-chain [`ObjectId`] from a 32-byte ed25519 public key.
///
/// In Opolys, a wallet address is the Blake3-256 hash of the ed25519 verifying
/// (public) key. This ensures that addresses are uniformly 32 bytes regardless
/// of the underlying signature scheme, and it decouples the address format
/// from the raw public key representation.
///
/// ```ignore
/// let address = ed25519_public_key_to_object_id(&verifying_key_bytes);
/// ```
pub fn ed25519_public_key_to_object_id(public_key: &[u8; 32]) -> ObjectId {
    hash_bytes_to_object_id(public_key)
}

/// Hash raw bytes and wrap the result as an [`ObjectId`].
///
/// Internal helper used by [`ed25519_public_key_to_object_id`]. Kept private
/// because the preferred public API always goes through a typed function that
/// documents what is being hashed.
fn hash_bytes_to_object_id(data: &[u8]) -> ObjectId {
    let hash = crate::hash::hash_with_domain(crate::hash::DOMAIN_OBJECT_ID, data);
    ObjectId(hash)
}

/// Verify an ed25519 signature against a message and public key.
///
/// Returns `true` if and only if the signature is valid. Any malformed input
/// (wrong key length, invalid point, wrong signature length) results in `false`
/// rather than an error; the call site should never panic on bad crypto data.
///
/// # Parameters
///
/// - `public_key`: 32-byte ed25519 verifying/public key
/// - `message`: the signed message bytes
/// - `signature`: 64-byte ed25519 signature
///
/// # Panics
///
/// This function never panics; all error cases are mapped to `false`.
pub fn verify_ed25519(public_key: &[u8], message: &[u8], signature: &[u8]) -> bool {
    let key_len_ok = (public_key.len() as u64).ct_eq(&32);
    let sig_len_ok = (signature.len() as u64).ct_eq(&64);

    let mut pk_bytes = [0u8; 32];
    for (dst, src) in pk_bytes.iter_mut().zip(public_key.iter().copied()) {
        *dst = src;
    }

    let mut sig_bytes = [0u8; 64];
    for (dst, src) in sig_bytes.iter_mut().zip(signature.iter().copied()) {
        *dst = src;
    }

    // Dalek validates the encoded point; invalid points are rejected.
    let verifying_key = match VerifyingKey::from_bytes(&pk_bytes) {
        Ok(vk) => vk,
        Err(_) => return bool::from(Choice::from(0)),
    };

    // Dalek represents signatures as value types, so construction is infallible.
    let dalek_sig = DalekSignature::from_bytes(&sig_bytes);
    let sig_ok = Choice::from(verifying_key.verify(message, &dalek_sig).is_ok() as u8);

    bool::from(key_len_ok & sig_len_ok & sig_ok)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use rand::TryRngCore;
    use rand::rngs::OsRng;

    fn generate_keypair() -> (SigningKey, ObjectId) {
        let mut seed = [0u8; 32];
        OsRng.try_fill_bytes(&mut seed).unwrap();
        let signing_key = SigningKey::from_bytes(&seed);
        let verifying_key = signing_key.verifying_key();
        let object_id = ed25519_public_key_to_object_id(verifying_key.as_bytes());
        (signing_key, object_id)
    }

    #[test]
    fn sign_and_verify() {
        let (signing_key, _) = generate_keypair();
        let message = b"test message";
        let signature: DalekSignature = signing_key.sign(message);
        let verifying_key = signing_key.verifying_key();

        assert!(verify_ed25519(
            verifying_key.as_bytes(),
            message,
            &signature.to_bytes(),
        ));
    }

    #[test]
    fn verify_wrong_message_fails() {
        let (signing_key, _) = generate_keypair();
        let message = b"test message";
        let signature: DalekSignature = signing_key.sign(message);
        let verifying_key = signing_key.verifying_key();

        assert!(!verify_ed25519(
            verifying_key.as_bytes(),
            b"wrong message",
            &signature.to_bytes(),
        ));
    }

    #[test]
    fn malformed_lengths_fail_without_panic() {
        let (signing_key, _) = generate_keypair();
        let message = b"test message";
        let signature: DalekSignature = signing_key.sign(message);
        let verifying_key = signing_key.verifying_key();

        assert!(!verify_ed25519(
            &verifying_key.as_bytes()[..31],
            message,
            &signature.to_bytes(),
        ));
        assert!(!verify_ed25519(
            verifying_key.as_bytes(),
            message,
            &signature.to_bytes()[..63],
        ));
    }

    #[test]
    fn object_id_from_public_key() {
        let (_, id1) = generate_keypair();
        let (_, id2) = generate_keypair();
        assert_ne!(id1, id2);
    }
}
