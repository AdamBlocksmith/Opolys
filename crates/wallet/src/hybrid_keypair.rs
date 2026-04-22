//! Hybrid classical + quantum-resistant key pair for dual-signature authentication.
//!
//! Opolys uses a hybrid signature scheme combining ed25519 (classical) and
//! Dilithium (post-quantum) signatures. Every transaction carries both
//! signatures. If a quantum computer breaks ed25519 in the future, the
//! Dilithium signature still provides security — ensuring forward-looking
//! protection for the $OPL blockchain.
//!
//! Production wallets derive keys deterministically from BIP-39 mnemonics
//! via `Bip39Mnemonic::derive_hybrid_keys()`. This module's `HybridKeypair::generate()`
//! is intended for testing only.

use ed25519_dalek::{SigningKey, Signer, Verifier};
use opolys_crypto::dilithium::{DilithiumKeypair, dilithium_verify};

/// A hybrid keypair combining classical (ed25519) and quantum-resistant
/// (Dilithium) keys for dual-signature protection.
///
/// In production, transactions carry both signatures. If a quantum computer
/// breaks ed25519, the Dilithium signature still provides security.
pub struct HybridKeypair {
    /// ed25519 signing key (32-byte seed).
    pub classical: SigningKey,
    /// Dilithium key pair (~2.4 KB public key, ~5 KB secret key).
    pub quantum: DilithiumKeypair,
}

impl HybridKeypair {
    /// Generate a random hybrid keypair.
    ///
    /// **Testing only** — production wallets should derive keys deterministically
    /// from a BIP-39 mnemonic via `Bip39Mnemonic::derive_hybrid_keys()`.
    pub fn generate() -> Self {
        let mut rng = rand::rngs::OsRng;
        let mut seed = [0u8; 32];
        use rand::TryRngCore;
        rng.try_fill_bytes(&mut seed).expect("Random generation failed");
        let classical = SigningKey::from_bytes(&seed);
        let quantum = DilithiumKeypair::generate();
        Self { classical, quantum }
    }

    /// Construct a `HybridKeypair` from an existing ed25519 seed and Dilithium keypair.
    ///
    /// Useful when both key components are already available (e.g. loaded from
    /// a wallet file).
    pub fn from_parts(ed25519_seed: &[u8; 32], quantum: DilithiumKeypair) -> Self {
        let classical = SigningKey::from_bytes(ed25519_seed);
        Self { classical, quantum }
    }

    /// Sign a message with the ed25519 (classical) private key.
    ///
    /// Returns a 64-byte signature.
    pub fn sign_classical(&self, message: &[u8]) -> Vec<u8> {
        let sig: ed25519_dalek::Signature = self.classical.sign(message);
        sig.to_bytes().to_vec()
    }

    /// Sign a message with the Dilithium (quantum-resistant) private key.
    ///
    /// Returns a ~2.4 KB signature.
    pub fn sign_quantum(&self, message: &[u8]) -> Vec<u8> {
        self.quantum.sign(message).to_vec()
    }

    /// Verify both the classical (ed25519) and quantum-resistant (Dilithium)
    /// components of a hybrid signature.
    ///
    /// Returns `true` only if **both** signatures are valid. This ensures
    /// quantum-resistant protection even if ed25519 is compromised.
    pub fn verify_hybrid(
        classical_pk: &[u8],
        quantum_pk: &[u8],
        message: &[u8],
        classical_sig: &[u8],
        quantum_sig: &[u8],
    ) -> bool {
        let classical_ok = if classical_pk.len() == 32 && classical_sig.len() == 64 {
            if let Ok(vk) = ed25519_dalek::VerifyingKey::from_bytes(
                <&[u8; 32]>::try_from(classical_pk).expect("Invalid pk len")
            ) {
                let sig_bytes: [u8; 64] = classical_sig.try_into().unwrap_or([0u8; 64]);
                let sig = ed25519_dalek::Signature::from_bytes(&sig_bytes);
                vk.verify(message, &sig).is_ok()
            } else {
                false
            }
        } else {
            false
        };

        let quantum_ok = dilithium_verify(quantum_sig, message, quantum_pk);

        classical_ok && quantum_ok
    }
}