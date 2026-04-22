use ed25519_dalek::{SigningKey, VerifyingKey, Signature as DalekSignature, Signer, Verifier};
use opolys_core::ObjectId;
use rand::TryRngCore;
use rand::rngs::OsRng;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use opolys_crypto::hash_to_object_id;

#[derive(Debug)]
pub enum WalletError {
    KeyGeneration(String),
    Signing(String),
    Verification(String),
    KeyNotFound(String),
    IoError(String),
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

#[derive(Debug, Clone)]
pub struct KeyPair {
    signing_key: SigningKey,
    verifying_key: VerifyingKey,
    object_id: ObjectId,
}

impl KeyPair {
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

    pub fn object_id(&self) -> &ObjectId {
        &self.object_id
    }

    pub fn verifying_key(&self) -> &VerifyingKey {
        &self.verifying_key
    }

    pub fn sign(&self, message: &[u8]) -> Vec<u8> {
        let signature: DalekSignature = self.signing_key.sign(message);
        signature.to_bytes().to_vec()
    }

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

    pub fn to_bytes(&self) -> [u8; 32] {
        self.signing_key.to_bytes()
    }

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

    pub fn public_key_bytes(&self) -> Vec<u8> {
        self.verifying_key.as_bytes().to_vec()
    }
}

pub struct Wallet {
    keys: HashMap<ObjectId, KeyPair>,
    keys_dir: PathBuf,
}

impl Wallet {
    pub fn new(keys_dir: PathBuf) -> Self {
        Self {
            keys: HashMap::new(),
            keys_dir,
        }
    }

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

    pub fn get_account(&self, object_id: &ObjectId) -> Option<&KeyPair> {
        self.keys.get(object_id)
    }

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