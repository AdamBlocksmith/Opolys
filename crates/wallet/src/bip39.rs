//! BIP-39 mnemonic and SLIP-0010 key derivation for the Opolys blockchain.
//!
//! Opolys uses 24-word BIP-39 mnemonics (256 bits of entropy) for wallet
//! recovery. From the mnemonic seed, ed25519 key pairs are derived via
//! SLIP-0010 at the path `m/44'/999'/0'/0'`, where:
//!
//! - `44'` — BIP-44 purpose for HD wallets
//! - `999'` — SLIP-0044 coin type for Opolys ($OPL)
//! - `0'` — account index
//! - `0'` — change index (unused, always 0 for ed25519)
//!
//! All derivation levels are hardened because ed25519 only supports hardened
//! child key derivation per SLIP-0010.
//!
//! The same ed25519 key is used for both transaction signing and validator
//! block signing. Wallet recovery from mnemonic alone restores all keys —
//! no separate backup file is needed.

use crate::key::{KeyPair, WalletError};
use hmac::{Hmac, Mac};
use sha2::Sha512;

type HmacSha512 = Hmac<Sha512>;

/// BIP-44 coin type for Opolys ($OPL), registered in SLIP-0044.
pub const OPOLYS_COIN_TYPE: u32 = 999;
/// BIP-44 purpose field (always 44' for BIP-44 HD wallets).
pub const BIP44_PURPOSE: u32 = 44;
/// Number of words in the mnemonic (24 = 256 bits of entropy).
pub const MNEMONIC_WORDS: usize = 24;

/// SLIP-0010 master key derivation key for ed25519.
const SLIP10_MASTER_KEY: &[u8] = b"ed25519 seed";

/// A BIP-39 mnemonic phrase, backed by the `bip39` crate.
///
/// Uses the full 2048-word English wordlist with proper checksum validation.
/// 24-word mnemonics provide 256 bits of entropy.
#[derive(Clone, Debug)]
pub struct Bip39Mnemonic {
    inner: bip39::Mnemonic,
}

impl Bip39Mnemonic {
    /// Generate a new random 24-word mnemonic (256 bits of entropy).
    pub fn generate() -> Self {
        let inner = bip39::Mnemonic::generate(MNEMONIC_WORDS)
            .expect("Failed to generate 24-word mnemonic");
        Bip39Mnemonic { inner }
    }

    /// Parse and validate an existing mnemonic phrase.
    ///
    /// Validates the BIP-39 checksum and ensures exactly 24 words. This rejects
    /// invalid mnemonics (bad checksum, wrong word count, unknown words).
    pub fn from_words(phrase: &str) -> Result<Self, WalletError> {
        let inner = bip39::Mnemonic::parse_normalized(phrase)
            .map_err(|e| WalletError::MnemonicError(format!("Invalid mnemonic: {}", e)))?;

        if inner.word_count() != MNEMONIC_WORDS {
            return Err(WalletError::MnemonicError(format!(
                "Expected {} words, got {}",
                MNEMONIC_WORDS,
                inner.word_count()
            )));
        }

        Ok(Bip39Mnemonic { inner })
    }

    /// The 24-word mnemonic phrase as a space-separated string.
    pub fn phrase(&self) -> String {
        self.inner.to_string()
    }

    /// Individual words of the mnemonic.
    pub fn words(&self) -> Vec<String> {
        self.inner.words().map(String::from).collect()
    }

    /// Derive the 64-byte seed using PBKDF2 with the standard BIP-39
    /// salt format ("mnemonic" + optional passphrase).
    pub fn to_seed(&self, passphrase: &str) -> DerivedSeed {
        let seed = self.inner.to_seed(passphrase);
        DerivedSeed { seed }
    }
}

/// The 64-byte seed derived from a BIP-39 mnemonic via PBKDF2.
///
/// This is the master secret from which all key material is derived
/// using SLIP-0010 for ed25519. The passphrase argument allows BIP-39
/// password protection for additional security.
#[derive(Debug)]
pub struct DerivedSeed {
    seed: [u8; 64],
}

impl DerivedSeed {
    /// The raw 64-byte seed.
    pub fn as_bytes(&self) -> &[u8; 64] {
        &self.seed
    }

    /// Derive an ed25519 keypair for the given account index using SLIP-0010.
    ///
    /// Path: m/44'/999'/account'/0'
    ///       └─ purpose=44, coin_type=999 (OPL), account, change=0
    ///
    /// SLIP-0010 is the standard for ed25519 HD derivation. All levels
    /// are hardened (≥ 0x80000000) because ed25519 only supports hardened
    /// child key derivation.
    ///
    /// The same key is used for both transaction signing and validator
    /// block signing. Full wallet recovery from mnemonic alone is supported.
    pub fn derive_keypair(&self, account: u32) -> KeyPair {
        let path = DerivationPath::new(BIP44_PURPOSE, OPOLYS_COIN_TYPE, account);
        let (private_key, _chain_code) = slip10_derive_ed25519(&self.seed, &path);
        KeyPair::from_seed(&private_key)
    }

    /// Derive an ed25519 keypair for the given account index.
    ///
    /// Alias for `derive_keypair()` for backward compatibility.
    pub fn derive_classical_keypair(&self, account: u32) -> KeyPair {
        self.derive_keypair(account)
    }
}

/// BIP-44 derivation path components.
///
/// Represents `m/44'/999'/account'` — all indices are hardened per SLIP-0010
/// requirements for ed25519.
struct DerivationPath {
    purpose: u32,
    coin_type: u32,
    account: u32,
}

impl DerivationPath {
    /// Create a derivation path with the given purpose, coin type, and account.
    fn new(purpose: u32, coin_type: u32, account: u32) -> Self {
        DerivationPath { purpose, coin_type, account }
    }

    /// Returns the hardened derivation indices for SLIP-0010.
    ///
    /// All levels are hardened (0x80000000 | value) because ed25519 only
    /// supports hardened child key derivation per SLIP-0010.
    fn indices(&self) -> Vec<u32> {
        vec![
            0x80000000 | self.purpose,   // 44'
            0x80000000 | self.coin_type,  // 999'
            0x80000000 | self.account,    // account'
            0x80000000 | 0,              // 0' (change)
        ]
    }
}

/// SLIP-0010 master key derivation for ed25519.
///
/// Derives the master secret key and chain code from the BIP-39 seed
/// using HMAC-SHA512 with the key "ed25519 seed".
fn slip10_master_key(seed: &[u8]) -> ([u8; 32], [u8; 32]) {
    let mut mac = HmacSha512::new_from_slice(SLIP10_MASTER_KEY)
        .expect("HMAC key should be valid");
    mac.update(seed);
    let result = mac.finalize().into_bytes();

    let mut private_key = [0u8; 32];
    private_key.copy_from_slice(&result[..32]);
    let mut chain_code = [0u8; 32];
    chain_code.copy_from_slice(&result[32..64]);

    (private_key, chain_code)
}

/// SLIP-0010 child key derivation for ed25519 (hardened only).
///
/// Each child: HMAC-SHA512(Key=parent_chain_code, Data=0x00 || parent_key || index_be)
fn slip10_derive_ed25519(seed: &[u8], path: &DerivationPath) -> ([u8; 32], [u8; 32]) {
    let (mut key, mut chain_code) = slip10_master_key(seed);

    for index in path.indices() {
        let mut mac = HmacSha512::new_from_slice(&chain_code)
            .expect("HMAC key should be valid");
        mac.update(&[0x00]);
        mac.update(&key);
        mac.update(&index.to_be_bytes());
        let result = mac.finalize().into_bytes();

        key.copy_from_slice(&result[..32]);
        chain_code.copy_from_slice(&result[32..64]);
    }

    (key, chain_code)
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
    fn mnemonic_phrase_roundtrip() {
        let mnemonic = Bip39Mnemonic::generate();
        let phrase = mnemonic.phrase();
        let restored = Bip39Mnemonic::from_words(&phrase).expect("Should restore mnemonic");
        assert_eq!(restored.phrase(), phrase);
    }

    #[test]
    fn seed_determinism() {
        let mnemonic = Bip39Mnemonic::generate();
        let seed1 = mnemonic.to_seed("");
        let seed2 = mnemonic.to_seed("");
        assert_eq!(seed1.as_bytes(), seed2.as_bytes());
    }

    #[test]
    fn different_passphrases_produce_different_seeds() {
        let mnemonic = Bip39Mnemonic::generate();
        let seed1 = mnemonic.to_seed("");
        let seed2 = mnemonic.to_seed("extra-passphrase");
        assert_ne!(seed1.as_bytes(), seed2.as_bytes());
    }

    #[test]
    fn derive_keypair_deterministic() {
        let mnemonic = Bip39Mnemonic::generate();
        let seed = mnemonic.to_seed("");
        let key1 = seed.derive_keypair(0);
        let key2 = seed.derive_keypair(0);
        assert_eq!(key1.object_id(), key2.object_id());
    }

    #[test]
    fn derive_different_accounts() {
        let mnemonic = Bip39Mnemonic::generate();
        let seed = mnemonic.to_seed("");
        let key0 = seed.derive_keypair(0);
        let key1 = seed.derive_keypair(1);
        assert_ne!(key0.object_id(), key1.object_id());
    }

    #[test]
    fn derive_classical_keypair_deterministic() {
        let mnemonic = Bip39Mnemonic::generate();
        let seed = mnemonic.to_seed("");
        let key1 = seed.derive_classical_keypair(0);
        let key2 = seed.derive_classical_keypair(0);
        assert_eq!(key1.object_id(), key2.object_id());
    }

    #[test]
    fn reject_wrong_word_count() {
        // 12-word mnemonic should be rejected (we require 24)
        let mnemonic_12 = bip39::Mnemonic::generate(12).expect("Should generate 12 words");
        let phrase = mnemonic_12.to_string();
        assert!(Bip39Mnemonic::from_words(&phrase).is_err());
    }

    #[test]
    fn slip10_derivation_produces_valid_ed25519_keys() {
        let mnemonic = Bip39Mnemonic::generate();
        let seed = mnemonic.to_seed("");
        let key = seed.derive_keypair(0);
        let msg = b"test message";
        let sig = key.sign(msg);
        assert!(key.verify(msg, &sig));
    }
}