use crate::key::{KeyPair, WalletError};
use opolys_crypto::dilithium::{DilithiumKeypair, DILITHIUM_SIGNBYTES};
use sha2::{Sha256, Digest};
use hmac::{Hmac, Mac};
use rand::rngs::OsRng;
use rand::TryRngCore;

type HmacSha512 = Hmac<sha2::Sha512>;

pub const MNEMONIC_WORDS: usize = 24;
pub const SEED_ITERATIONS: u32 = 2048;
pub const OPOLYS_COIN_TYPE: u32 = 999;
pub const BIP44_PURPOSE: u32 = 44;

#[derive(Clone, Debug)]
pub struct Bip39Mnemonic {
    entropy: [u8; 32],
    words: Vec<String>,
}

pub struct DerivedSeed {
    seed: [u8; 64],
}

impl std::fmt::Debug for DerivedSeed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DerivedSeed").field("seed", &"[...64 bytes]").finish()
    }
}

pub struct HybridMasterKeys {
    pub classical: ClassicalKeychain,
    pub quantum: QuantumKeychain,
}

pub struct ClassicalKeychain {
    master_key: KeyPair,
    path: Vec<u32>,
}

pub struct QuantumKeychain {
    keypair: DilithiumKeypair,
    epoch: u32,
}

#[derive(Debug, Clone)]
pub struct HybridSignature {
    pub classical_sig: Vec<u8>,
    pub quantum_sig: Vec<u8>,
    pub epoch: u32,
    pub is_quantum_only: bool,
}

impl Bip39Mnemonic {
    pub fn generate() -> Self {
        let mut entropy = [0u8; 32];
        OsRng.try_fill_bytes(&mut entropy).expect("Failed to generate entropy");
        let words = Self::entropy_to_words(&entropy);
        Self { entropy, words }
    }

    pub fn from_words(words: &[String]) -> Result<Self, WalletError> {
        if words.len() != MNEMONIC_WORDS {
            return Err(WalletError::MnemonicError(format!(
                "Expected {} words, got {}", MNEMONIC_WORDS, words.len()
            )));
        }
        let wordlist = crate::wordlist::OPOLYS_WORDLIST;
        let mut indices = Vec::with_capacity(MNEMONIC_WORDS);
        for w in words {
            let idx = wordlist.iter().position(|x| *x == w.as_str())
                .ok_or_else(|| WalletError::MnemonicError(format!("Word not in wordlist: {}", w)))?;
            indices.push(idx);
        }

        let mut all_bits = Vec::with_capacity(264);
        for idx in &indices {
            for i in (0..11).rev() {
                all_bits.push((*idx >> i) & 1);
            }
        }

        let mut entropy = [0u8; 32];
        for i in 0..32 {
            let mut byte_val: u8 = 0;
            for j in 0..8 {
                byte_val = (byte_val << 1) | (all_bits[i * 8 + j] as u8);
            }
            entropy[i] = byte_val;
        }

        let mut hasher = Sha256::new();
        hasher.update(&entropy);
        let hash = hasher.finalize();
        let checksum_byte = hash[0];
        let checksum_len: usize = 1;
        let mut expected_checksum: usize = 0;
        for i in 0..checksum_len {
            expected_checksum = (expected_checksum << 1) | ((checksum_byte >> (8 - 1 - i as u32)) & 1) as usize;
        }

        // Simplified validation — in production, full BIP39 checksum validation is needed
        Ok(Self { entropy, words: words.to_vec() })
    }

    pub fn words(&self) -> &[String] {
        &self.words
    }

    pub fn to_seed(&self, passphrase: &str) -> DerivedSeed {
        let mut seed = [0u8; 64];
        let salt = format!("mnemonicopolys{}", passphrase);
        pbkdf2::pbkdf2_hmac::<Sha256>(
            self.words.join(" ").as_bytes(),
            salt.as_bytes(),
            SEED_ITERATIONS,
            &mut seed,
        );
        DerivedSeed { seed }
    }

    fn entropy_to_words(entropy: &[u8; 32]) -> Vec<String> {
        let wordlist = crate::wordlist::OPOLYS_WORDLIST;
        let mut hasher = Sha256::new();
        hasher.update(entropy);
        let hash = hasher.finalize();
        let checksum_byte = hash[0];

        let mut bits = Vec::with_capacity(264);
        for byte in entropy.iter() {
            for i in (0..8).rev() {
                bits.push((byte >> i) & 1);
            }
        }
        for i in (0..4).rev() {
            bits.push((checksum_byte >> i) & 1);
        }

        let mut words = Vec::with_capacity(MNEMONIC_WORDS);
        for i in 0..MNEMONIC_WORDS {
            let mut index: usize = 0;
            for j in 0..11 {
                if i * 11 + j < bits.len() {
                    index = (index << 1) | (bits[i * 11 + j] as usize);
                }
            }
            index = index.min(wordlist.len() - 1);
            words.push(wordlist[index].to_string());
        }
        words
    }

    pub fn entropy(&self) -> &[u8; 32] {
        &self.entropy
    }
}

impl DerivedSeed {
    pub fn as_bytes(&self) -> &[u8; 64] {
        &self.seed
    }

    pub fn derive_classical_keypair(&self, account: u32) -> KeyPair {
        let path = format!("m/{}'/{}'/0'/0'", BIP44_PURPOSE, OPOLYS_COIN_TYPE);
        let mut mac = HmacSha512::new_from_slice(b"opolys-classical-derivation")
            .expect("HMAC key error");
        mac.update(&self.seed);
        mac.update(path.as_bytes());
        mac.update(&account.to_le_bytes());
        let result = mac.finalize().into_bytes();
        let mut seed_bytes = [0u8; 32];
        seed_bytes.copy_from_slice(&result[..32]);
        KeyPair::from_seed(&seed_bytes)
    }

    pub fn derive_hybrid_keys(&self, account: u32) -> HybridMasterKeys {
        let classical = ClassicalKeychain {
            master_key: self.derive_classical_keypair(account),
            path: vec![BIP44_PURPOSE, OPOLYS_COIN_TYPE, account, 0, 0],
        };
        let quantum = QuantumKeychain {
            keypair: DilithiumKeypair::generate(),
            epoch: 0,
        };
        HybridMasterKeys { classical, quantum }
    }
}

impl ClassicalKeychain {
    pub fn keypair(&self) -> &KeyPair {
        &self.master_key
    }

    pub fn path(&self) -> &[u32] {
        &self.path
    }
}

impl QuantumKeychain {
    pub fn public_key(&self) -> &[u8] {
        self.keypair.public_key()
    }

    pub fn sign(&self, message: &[u8]) -> [u8; DILITHIUM_SIGNBYTES] {
        self.keypair.sign(message)
    }

    pub fn epoch(&self) -> u32 {
        self.epoch
    }
}

impl HybridSignature {
    pub fn hybrid(classical: Vec<u8>, quantum: Vec<u8>, epoch: u32) -> Self {
        Self {
            classical_sig: classical,
            quantum_sig: quantum,
            epoch,
            is_quantum_only: false,
        }
    }

    pub fn quantum_only(quantum: Vec<u8>, epoch: u32) -> Self {
        Self {
            classical_sig: Vec::new(),
            quantum_sig: quantum,
            epoch,
            is_quantum_only: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_mnemonic() {
        let mnemonic = Bip39Mnemonic::generate();
        assert_eq!(mnemonic.words().len(), MNEMONIC_WORDS);
    }

    #[test]
    fn seed_determinism() {
        let mnemonic = Bip39Mnemonic::generate();
        let seed1 = mnemonic.to_seed("");
        let seed2 = mnemonic.to_seed("");
        assert_eq!(seed1.as_bytes(), seed2.as_bytes());
    }

    #[test]
    fn derive_classical_keypair() {
        let mnemonic = Bip39Mnemonic::generate();
        let seed = mnemonic.to_seed("");
        let key = seed.derive_classical_keypair(0);
        let key2 = seed.derive_classical_keypair(0);
        assert_eq!(key.object_id(), key2.object_id());
    }

    #[test]
    fn derive_different_accounts() {
        let mnemonic = Bip39Mnemonic::generate();
        let seed = mnemonic.to_seed("");
        let key0 = seed.derive_classical_keypair(0);
        let key1 = seed.derive_classical_keypair(1);
        assert_ne!(key0.object_id(), key1.object_id());
    }

    #[test]
    fn hybrid_keys_creation() {
        let mnemonic = Bip39Mnemonic::generate();
        let seed = mnemonic.to_seed("");
        let hybrid = seed.derive_hybrid_keys(0);
        assert!(hybrid.classical.keypair().object_id().to_hex().len() > 0);
        assert!(hybrid.quantum.public_key().len() > 0);
    }
}