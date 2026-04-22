//! Core key pair and wallet primitives for the Opolys blockchain.
//!
//! Provides `KeyPair` (ed25519-based) and `Wallet` (multi-key manager) built on
//! Blake3-256 hashing. ObjectIds (addresses) are derived as Blake3(public_key_bytes),
//! ensuring a direct cryptographic link between identity and key material.
//!
//! For deterministic key derivation from mnemonics, see the `bip39` module.
//! For hybrid classical + quantum-resistant signing, see `hybrid_keypair`.

use ed25519_dalek::{SigningKey, VerifyingKey, Signature as DalekSignature, Signer, Verifier};
use opolys_core::ObjectId;
use rand::TryRngCore;
use rand::rngs::OsRng;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use opolys_crypto::hash_to_object_id;

/// Errors that can occur during wallet operations.
#[derive(Debug)]
pub enum WalletError {
    /// Failed to generate a key pair.
    KeyGeneration(String),
    /// Failed to create or verify a signature.
    Signing(String),
    /// Signature verification failed.
    Verification(String),
    /// The requested key was not found in the wallet.
    KeyNotFound(String),
    /// Filesystem I/O error (e.g. reading/writing key files).
    IoError(String),
    /// BIP-39 mnemonic parsing or validation error.
    MnemonicError(String),
}

impl std::fmt::Display for WalletError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WalletError::KeyGeneration(s) => write!(f, "Key generation error: {}", s),
            WalletError::Signing(s) => write!(f, "Signing error: {}", s),
            WalletError::Verification(s) => write!(f, "Verification error: {}", s),
            WalletError::KeyNotFound(s) => write!(f, "Key not found: {}", s),
            WalletError::IoError(s) => write!(f, "IO error: {}", s),
            WalletError::MnemonicError(s) => write!(f, "Mnemonic error: {}", s),
        }
    }
}

impl std::error::Error for WalletError {}

/// An ed25519 key pair for the Opolys blockchain.
///
/// The `ObjectId` (account address) is deterministically derived from the
/// verifying (public) key via Blake3-256 hashing. This ensures every public
/// key maps to a unique on-chain identity.
///
/// Use `KeyPair::generate()` for random keys or `KeyPair::from_seed()` for
/// deterministic keys derived from SLIP-0010 HD paths.
#[derive(Debug, Clone)]
pub struct KeyPair {
    signing_key: SigningKey,
    verifying_key: VerifyingKey,
    /// Blake3-256 hash of the public key — this is the on-chain identity.
    object_id: ObjectId,
}

impl KeyPair {
    /// Generate a random ed25519 key pair using the OS CSPRNG.
    ///
    /// The private key seed is filled with `OsRng`. The `ObjectId` is
    /// computed as `Blake3(verifying_key_bytes)`.
    pub fn generate() -> Self {
        let mut rng = OsRng;
        let mut seed = [0u8; 32];
        rng.try_fill_bytes(&mut seed).expect("Failed to generate random bytes");
        let signing_key = SigningKey::from_bytes(&seed);
        let verifying_key = signing_key.verifying_key();
        let object_id = hash_to_object_id(verifying_key.as_bytes());
        Self {
            signing_key,
            verifying_key,
            object_id,
        }
    }

    /// Reconstruct a key pair from a 32-byte seed (e.g. from SLIP-0010 derivation).
    ///
    /// The seed is the raw ed25519 private key. The `ObjectId` is recomputed
    /// deterministically from the resulting public key.
    pub fn from_seed(seed: &[u8; 32]) -> Self {
        let signing_key = SigningKey::from_bytes(seed);
        let verifying_key = signing_key.verifying_key();
        let object_id = hash_to_object_id(verifying_key.as_bytes());
        Self {
            signing_key,
            verifying_key,
            object_id,
        }
    }

    /// The on-chain identity (Blake3-256 hash of the public key).
    pub fn object_id(&self) -> &ObjectId {
        &self.object_id
    }

    /// The ed25519 verifying (public) key.
    pub fn verifying_key(&self) -> &VerifyingKey {
        &self.verifying_key
    }

    /// Sign a message with the ed25519 private key.
    pub fn sign(&self, message: &[u8]) -> Vec<u8> {
        let signature: DalekSignature = self.signing_key.sign(message);
        signature.to_bytes().to_vec()
    }

    /// Verify an ed25519 signature against this key pair's public key.
    ///
    /// Returns `false` for invalid signatures, wrong message, or
    /// incorrectly-sized signature bytes.
    pub fn verify(&self, message: &[u8], signature: &[u8]) -> bool {
        if signature.len() != 64 {
            return false;
        }
        let sig_bytes: [u8; 64] = match signature.try_into() {
            Ok(b) => b,
            Err(_) => return false,
        };
        let dalek_sig = DalekSignature::from_bytes(&sig_bytes);
        self.verifying_key.verify(message, &dalek_sig).is_ok()
    }

    /// Serialize the 32-byte private key seed.
    pub fn to_bytes(&self) -> [u8; 32] {
        self.signing_key.to_bytes()
    }

    /// Reconstruct a key pair from a 32-byte seed.
    ///
    /// Returns `None` if the input is not exactly 32 bytes (should not happen
    /// with the `[u8; 32]` type, but included for API consistency).
    pub fn from_bytes(bytes: &[u8; 32]) -> Option<Self> {
        let signing_key = SigningKey::from_bytes(bytes);
        let verifying_key = signing_key.verifying_key();
        let object_id = hash_to_object_id(verifying_key.as_bytes());
        Some(Self {
            signing_key,
            verifying_key,
            object_id,
        })
    }

    /// The 32-byte ed25519 public key (compressed point).
    pub fn public_key_bytes(&self) -> Vec<u8> {
        self.verifying_key.as_bytes().to_vec()
    }
}

/// Multi-key wallet that manages named ed25519 key pairs on disk.
///
/// Each key pair is stored as a raw 32-byte seed file under `keys_dir/{name}_{short_id}.key`.
/// The short ID is the first 16 hex characters of the `ObjectId`, providing a
/// human-readable yet unique filename that avoids collisions.
pub struct Wallet {
    keys: HashMap<ObjectId, KeyPair>,
    keys_dir: PathBuf,
}

impl Wallet {
    /// Create a new (empty) wallet that reads/writes keys to `keys_dir`.
    pub fn new(keys_dir: PathBuf) -> Self {
        Self {
            keys: HashMap::new(),
            keys_dir,
        }
    }

    /// Generate a new key pair, persist its seed to disk, and add it to the wallet.
    ///
    /// The key file is named `{name}_{short_id}.key` to avoid filename conflicts
    /// while staying human-readable. Returns the `ObjectId` for the new key.
    pub fn create_account(&mut self, name: &str) -> Result<ObjectId, WalletError> {
        let keypair = KeyPair::generate();
        let object_id = keypair.object_id().clone();

        fs::create_dir_all(&self.keys_dir)
            .map_err(|e| WalletError::IoError(e.to_string()))?;

        let key_path = self.keys_dir.join(format!("{}_{}.key", name, object_id.to_hex()[..16].to_string()));
        let key_bytes = keypair.to_bytes();
        fs::write(&key_path, &key_bytes)
            .map_err(|e| WalletError::IoError(e.to_string()))?;

        self.keys.insert(object_id.clone(), keypair);
        Ok(object_id)
    }

    /// Look up a key pair by its on-chain `ObjectId`.
    pub fn get_account(&self, object_id: &ObjectId) -> Option<&KeyPair> {
        self.keys.get(object_id)
    }

    /// List all `ObjectId`s held in this wallet.
    pub fn list_accounts(&self) -> Vec<ObjectId> {
        self.keys.keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_keypair() {
        let keypair = KeyPair::generate();
        assert!(!keypair.object_id().to_hex().is_empty());
    }

    #[test]
    fn sign_and_verify() {
        let keypair = KeyPair::generate();
        let message = b"test message";
        let signature = keypair.sign(message);
        assert!(keypair.verify(message, &signature));
    }

    #[test]
    fn verify_wrong_message_fails() {
        let keypair = KeyPair::generate();
        let signature = keypair.sign(b"original");
        assert!(!keypair.verify(b"tampered", &signature));
    }

    #[test]
    fn keypair_deterministic_from_seed() {
        let seed = [42u8; 32];
        let kp1 = KeyPair::from_seed(&seed);
        let kp2 = KeyPair::from_seed(&seed);
        assert_eq!(kp1.object_id(), kp2.object_id());
    }
}