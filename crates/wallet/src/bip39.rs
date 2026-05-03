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
//! The same ed25519 key is used for both transaction signing and refiner
//! block signing. Wallet recovery from mnemonic alone restores all keys —
//! no separate backup file is needed.

use crate::key::{KeyPair, WalletError};
use hmac::{Hmac, Mac};
use sha2::Sha512;
use std::fmt;
use zeroize::Zeroize;

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
#[derive(Clone)]
pub struct Bip39Mnemonic {
    inner: bip39::Mnemonic,
}

impl fmt::Debug for Bip39Mnemonic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Bip39Mnemonic").field(&"[REDACTED]").finish()
    }
}

impl Bip39Mnemonic {
    /// Generate a new random 24-word mnemonic (256 bits of entropy).
    pub fn generate() -> Self {
        let inner =
            bip39::Mnemonic::generate(MNEMONIC_WORDS).expect("Failed to generate 24-word mnemonic");
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
pub struct DerivedSeed {
    seed: [u8; 64],
}

impl fmt::Debug for DerivedSeed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("DerivedSeed").field(&"[REDACTED]").finish()
    }
}

impl Drop for DerivedSeed {
    fn drop(&mut self) {
        self.seed.zeroize();
    }
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
    /// The same key is used for both transaction signing and refiner
    /// block signing. Full wallet recovery from mnemonic alone is supported.
    pub fn derive_keypair(&self, account: u32) -> KeyPair {
        let path = DerivationPath::new(BIP44_PURPOSE, OPOLYS_COIN_TYPE, account);
        let (mut private_key, mut chain_code) = slip10_derive_ed25519(&self.seed, &path);
        let keypair = KeyPair::from_seed(&private_key);
        private_key.zeroize();
        chain_code.zeroize();
        keypair
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
        DerivationPath {
            purpose,
            coin_type,
            account,
        }
    }

    /// Returns the hardened derivation indices for SLIP-0010.
    ///
    /// All levels are hardened (0x80000000 | value) because ed25519 only
    /// supports hardened child key derivation per SLIP-0010.
    fn indices(&self) -> Vec<u32> {
        vec![
            0x80000000 | self.purpose,   // 44'
            0x80000000 | self.coin_type, // 999'
            0x80000000 | self.account,   // account'
            0x80000000 | 0,              // 0' (change)
        ]
    }
}

/// SLIP-0010 master key derivation for ed25519.
///
/// Derives the master secret key and chain code from the BIP-39 seed
/// using HMAC-SHA512 with the key "ed25519 seed".
fn slip10_master_key(seed: &[u8]) -> ([u8; 32], [u8; 32]) {
    let mut mac = HmacSha512::new_from_slice(SLIP10_MASTER_KEY).expect("HMAC key should be valid");
    mac.update(seed);
    let mut result = mac.finalize().into_bytes();

    let mut private_key = [0u8; 32];
    private_key.copy_from_slice(&result[..32]);
    let mut chain_code = [0u8; 32];
    chain_code.copy_from_slice(&result[32..64]);
    result.zeroize();

    (private_key, chain_code)
}

/// SLIP-0010 child key derivation for ed25519 (hardened only).
///
/// Each child: HMAC-SHA512(Key=parent_chain_code, Data=0x00 || parent_key || index_be)
fn slip10_derive_ed25519(seed: &[u8], path: &DerivationPath) -> ([u8; 32], [u8; 32]) {
    slip10_derive_ed25519_indices(seed, &path.indices())
}

fn slip10_derive_ed25519_indices(seed: &[u8], indices: &[u32]) -> ([u8; 32], [u8; 32]) {
    let (mut key, mut chain_code) = slip10_master_key(seed);

    for &index in indices {
        let mut mac = HmacSha512::new_from_slice(&chain_code).expect("HMAC key should be valid");
        mac.update(&[0x00]);
        mac.update(&key);
        mac.update(&index.to_be_bytes());
        let mut result = mac.finalize().into_bytes();

        key.zeroize();
        chain_code.zeroize();
        key.copy_from_slice(&result[..32]);
        chain_code.copy_from_slice(&result[32..64]);
        result.zeroize();
    }

    (key, chain_code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;

    fn decode_hex_array<const N: usize>(hex: &str) -> [u8; N] {
        let bytes = hex::decode(hex).expect("valid hex test vector");
        bytes.try_into().expect("test vector has expected length")
    }

    fn slip10_public_hex(private_key: &[u8; 32]) -> String {
        let signing_key = SigningKey::from_bytes(private_key);
        format!("00{}", hex::encode(signing_key.verifying_key().as_bytes()))
    }

    fn assert_slip10_vector(
        seed_hex: &str,
        indices: &[u32],
        expected_private_hex: &str,
        expected_chain_code_hex: &str,
        expected_public_hex: &str,
    ) {
        let seed = hex::decode(seed_hex).expect("valid seed hex");
        let (private_key, chain_code) = slip10_derive_ed25519_indices(&seed, indices);

        assert_eq!(hex::encode(private_key), expected_private_hex);
        assert_eq!(hex::encode(chain_code), expected_chain_code_hex);
        assert_eq!(slip10_public_hex(&private_key), expected_public_hex);
    }

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
    fn debug_redacts_mnemonic_and_seed() {
        let mnemonic = Bip39Mnemonic::generate();
        let phrase = mnemonic.phrase();
        let seed = mnemonic.to_seed("");

        let mnemonic_debug = format!("{:?}", mnemonic);
        let seed_debug = format!("{:?}", seed);

        assert!(mnemonic_debug.contains("[REDACTED]"));
        assert!(!mnemonic_debug.contains(&phrase));
        assert!(seed_debug.contains("[REDACTED]"));
        assert!(!seed_debug.contains("seed"));
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

    #[test]
    fn slip10_official_ed25519_test_vector_1() {
        // Source: SLIP-0010 "Test vector 1 for ed25519".
        let seed = "000102030405060708090a0b0c0d0e0f";

        let (master_private, master_chain) = slip10_master_key(&decode_hex_array::<16>(seed));
        assert_eq!(
            hex::encode(master_private),
            "2b4be7f19ee27bbf30c667b642d5f4aa69fd169872f8fc3059c08ebae2eb19e7"
        );
        assert_eq!(
            hex::encode(master_chain),
            "90046a93de5380a72b5e45010748567d5ea02bbf6522f979e05c0d8d8ca9fffb"
        );
        assert_eq!(
            slip10_public_hex(&master_private),
            "00a4b2856bfec510abab89753fac1ac0e1112364e7d250545963f135f2a33188ed"
        );

        assert_slip10_vector(
            seed,
            &[0x80000000],
            "68e0fe46dfb67e368c75379acec591dad19df3cde26e63b93a8e704f1dade7a3",
            "8b59aa11380b624e81507a27fedda59fea6d0b779a778918a2fd3590e16e9c69",
            "008c8a13df77a28f3445213a0f432fde644acaa215fc72dcdf300d5efaa85d350c",
        );
        assert_slip10_vector(
            seed,
            &[0x80000000, 0x80000001],
            "b1d0bad404bf35da785a64ca1ac54b2617211d2777696fbffaf208f746ae84f2",
            "a320425f77d1b5c2505a6b1b27382b37368ee640e3557c315416801243552f14",
            "001932a5270f335bed617d5b935c80aedb1a35bd9fc1e31acafd5372c30f5c1187",
        );
        assert_slip10_vector(
            seed,
            &[0x80000000, 0x80000001, 0x80000002, 0x80000002, 0xbb9aca00],
            "8f94d394a8e8fd6b1bc2f3f49f5c47e385281d5c17e65324b0f62483e37e8793",
            "68789923a0cac2cd5a29172a475fe9e0fb14cd6adb5ad98a3fa70333e7afa230",
            "003c24da049451555d51a7014a37337aa4e12d41e485abccfa46b47dfb2af54b7a",
        );
    }

    #[test]
    fn slip10_official_ed25519_test_vector_2() {
        // Source: SLIP-0010 "Test vector 2 for ed25519".
        let seed = "fffcf9f6f3f0edeae7e4e1dedbd8d5d2cfccc9c6c3c0bdbab7b4b1aeaba8a5a29f9c999693908d8a8784817e7b7875726f6c696663605d5a5754514e4b484542";

        let (master_private, master_chain) = slip10_master_key(&decode_hex_array::<64>(seed));
        assert_eq!(
            hex::encode(master_private),
            "171cb88b1b3c1db25add599712e36245d75bc65a1a5c9e18d76f9f2b1eab4012"
        );
        assert_eq!(
            hex::encode(master_chain),
            "ef70a74db9c3a5af931b5fe73ed8e1a53464133654fd55e7a66f8570b8e33c3b"
        );
        assert_eq!(
            slip10_public_hex(&master_private),
            "008fe9693f8fa62a4305a140b9764c5ee01e455963744fe18204b4fb948249308a"
        );

        assert_slip10_vector(
            seed,
            &[0x80000000],
            "1559eb2bbec5790b0c65d8693e4d0875b1747f4970ae8b650486ed7470845635",
            "0b78a3226f915c082bf118f83618a618ab6dec793752624cbeb622acb562862d",
            "0086fab68dcb57aa196c77c5f264f215a112c22a912c10d123b0d03c3c28ef1037",
        );
        assert_slip10_vector(
            seed,
            &[0x80000000, 0xffffffff],
            "ea4f5bfe8694d8bb74b7b59404632fd5968b774ed545e810de9c32a4fb4192f4",
            "138f0b2551bcafeca6ff2aa88ba8ed0ed8de070841f0c4ef0165df8181eaad7f",
            "005ba3b9ac6e90e83effcd25ac4e58a1365a9e35a3d3ae5eb07b9e4d90bcf7506d",
        );
        assert_slip10_vector(
            seed,
            &[0x80000000, 0xffffffff, 0x80000001, 0xfffffffe, 0x80000002],
            "551d333177df541ad876a60ea71f00447931c0a9da16f227c11ea080d7391b8d",
            "5d70af781f3a37b829f0d060924d5e960bdc02e85423494afc0b1a41bbe196d4",
            "0047150c75db263559a70d5778bf36abbab30fb061ad69f69ece61a72b0cfa4fc0",
        );
    }
}
