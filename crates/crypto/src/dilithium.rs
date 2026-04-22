//! Dilithium (ML-DSA) post-quantum digital signatures for the Opolys blockchain.
//!
//! While ed25519 secures wallet operations today, Dilithium provides
//! quantum-resistant signatures as a forward-looking complement. Opolys adopts
//! a dual-signature model so that when large-scale quantum computers become
//! feasible, Dilithium-signed transactions will already be first-class citizens
//! on the network.
//!
//! **Key sizes** (Dilithium-5, the strongest parameter set):
//!
//! | Item                | Size     |
//! |---------------------|----------|
//! | Public key          | 2 592 B  |
//! | Secret key          | 4 864 B  |
//! | Signature           | 4 595 B  |
//!
//! Deterministic key generation from a seed (needed for BIP39-based wallet
//! recovery) is not yet available — the underlying `pqc_dilithium` crate
//! (v0.2) does not expose seeded key generation in its public API. Once it
//! does, `DilithiumKeypair::from_seed()` will be added for full wallet
//! recovery parity with ed25519.

/// Dilithium (ML-DSA) public key size in bytes (2 592 B for Dilithium-5).
pub const DILITHIUM_PUBLIC_KEY_BYTES: usize = pqc_dilithium::PUBLICKEYBYTES;
/// Dilithium (ML-DSA) secret key size in bytes (4 864 B for Dilithium-5).
pub const DILITHIUM_SECRET_KEY_BYTES: usize = pqc_dilithium::SECRETKEYBYTES;
/// Dilithium (ML-DSA) signature size in bytes (4 595 B for Dilithium-5).
pub const DILITHIUM_SIGNBYTES: usize = pqc_dilithium::SIGNBYTES;

/// A Dilithium (ML-DSA) keypair for post-quantum digital signatures.
///
/// Currently only supports random generation. Deterministic generation from
/// a seed awaits a crate upgrade (pqc_dilithium 0.2 doesn't expose seeded
/// key generation in its public API). Once available, this will be added
/// as `from_seed()` for full wallet recovery support.
pub struct DilithiumKeypair {
    inner: pqc_dilithium::Keypair,
}

impl DilithiumKeypair {
    /// Generate a new random Dilithium keypair using the system RNG.
    ///
    /// The secret key is generated with OS-provided randomness. This is
    /// suitable for creating one-time keypairs, but **not** for wallet
    /// recovery — use `from_seed()` (when available) for mnemonic-derived
    /// keypairs.
    pub fn generate() -> Self {
        DilithiumKeypair {
            inner: pqc_dilithium::Keypair::generate(),
        }
    }

    /// The Dilithium public key bytes (for sharing with verifiers).
    ///
    /// Length is [`DILITHIUM_PUBLIC_KEY_BYTES`] (2 592 bytes for Dilithium-5).
    pub fn public_key(&self) -> &[u8] {
        &self.inner.public
    }

    /// The Dilithium secret key bytes (for signing). Handle with care.
    ///
    /// Length is [`DILITHIUM_SECRET_KEY_BYTES`] (4 864 bytes for Dilithium-5).
    /// **Never** store or transmit these bytes unencrypted.
    pub fn secret_key(&self) -> &[u8] {
        self.inner.expose_secret()
    }

    /// Sign a message with the Dilithium secret key.
    ///
    /// Returns a fixed-length signature of [`DILITHIUM_SIGNBYTES`] bytes
    /// (4 595 bytes for Dilithium-5). The signature can be verified with
    /// [`dilithium_verify`] using the corresponding public key.
    pub fn sign(&self, message: &[u8]) -> [u8; DILITHIUM_SIGNBYTES] {
        self.inner.sign(message)
    }
}

/// Verify a Dilithium signature against a public key and message.
///
/// Returns `true` if and only if the signature is valid for the given message
/// and public key. Malformed inputs (wrong lengths, etc.) result in `false`
/// rather than an error, consistent with the verification pattern used
/// throughout Opolys.
///
/// # Parameters
///
/// - `signature`  — a [`DILITHIUM_SIGNBYTES`]-byte Dilithium signature
/// - `message`    — the signed message bytes
/// - `public_key` — a [`DILITHIUM_PUBLIC_KEY_BYTES`]-byte Dilithium public key
pub fn dilithium_verify(signature: &[u8], message: &[u8], public_key: &[u8]) -> bool {
    pqc_dilithium::verify(signature, message, public_key).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_keypair() {
        let kp = DilithiumKeypair::generate();
        assert_eq!(kp.public_key().len(), DILITHIUM_PUBLIC_KEY_BYTES);
        assert_eq!(kp.secret_key().len(), DILITHIUM_SECRET_KEY_BYTES);
    }

    #[test]
    fn sign_and_verify() {
        let kp = DilithiumKeypair::generate();
        let msg = b"test message";
        let sig = kp.sign(msg);
        assert!(dilithium_verify(&sig, msg, kp.public_key()));
    }

    #[test]
    fn verify_wrong_message_fails() {
        let kp = DilithiumKeypair::generate();
        let sig = kp.sign(b"original");
        assert!(!dilithium_verify(&sig, b"tampered", kp.public_key()));
    }
}