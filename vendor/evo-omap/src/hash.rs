//! # EVO-OMAP Hash Functions
//!
//! This module provides cryptographic hash functions used by the EVO-OMAP algorithm.
//!
//! ## Hash Functions
//!
//! - **Blake3-256**: Fast, parallelizable hash function used for inner hashing
//! - **SHA3-256**: Cryptographic hash based on Keccak sponge construction
//! - **Blake3-XOF**: Extendable output function for arbitrary-length outputs

use blake3::Hasher;
use sha3::Digest;

/// Size of a standard hash output in bytes (256 bits).
pub const HASH_SIZE: usize = 32;

/// A 256-bit hash value (32 bytes).
///
/// Used throughout EVO-OMAP for:
/// - Epoch seeds
/// - Mining seeds
/// - State summaries
/// - Memory commitments
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Hash(pub [u8; HASH_SIZE]);

impl Hash {
    /// Returns a reference to the underlying 32-byte array.
    pub fn as_bytes(&self) -> &[u8; HASH_SIZE] {
        &self.0
    }

    /// Creates a Hash from a 32-byte array.
    pub fn from_bytes(bytes: [u8; HASH_SIZE]) -> Self {
        Self(bytes)
    }
}

impl AsRef<[u8]> for Hash {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl From<[u8; HASH_SIZE]> for Hash {
    fn from(arr: [u8; HASH_SIZE]) -> Self {
        Self(arr)
    }
}

/// Computes Blake3-256 hash of the input data.
///
/// Blake3 is a fast, parallelizable hash function based on a Merkle tree
/// construction. It is used in EVO-OMAP for:
/// - Inner hash operations
/// - Dataset generation
/// - Rolling commitment
///
/// # Arguments
/// * `data` - Input data to hash
///
/// # Returns
/// A 32-byte Hash value
pub fn blake3_256(data: &[u8]) -> Hash {
    let hash = blake3::hash(data);
    let mut arr = [0u8; HASH_SIZE];
    arr.copy_from_slice(hash.as_bytes());
    Hash(arr)
}

/// Computes SHA3-256 hash of the input data.
///
/// SHA3-256 is based on the Keccak sponge construction and provides
/// cryptographic diversity from Blake3. It is used as the final
/// hash function in EVO-OMAP to produce the proof-of-work output.
///
/// # Arguments
/// * `data` - Input data to hash
///
/// # Returns
/// A 32-byte Hash value
pub fn sha3_256(data: &[u8]) -> Hash {
    let mut hasher = sha3::Sha3_256::new();
    hasher.update(data);
    let result = hasher.finalize();
    let mut arr = [0u8; HASH_SIZE];
    arr.copy_from_slice(&result);
    Hash(arr)
}

/// Generates arbitrary-length output from Blake3 using XOF mode.
///
/// The Extendable Output Function (XOF) allows generating any number
/// of bytes from a fixed-size input. Used in EVO-OMAP for:
/// - Dataset node generation (1 MiB per node)
/// - Cache block extension (64 KiB per block)
/// - Branch state derivation (64 bytes)
///
/// # Arguments
/// * `input` - Input seed data
/// * `output_len` - Desired number of output bytes
///
/// # Returns
/// Vector of `output_len` bytes
pub fn blake3_xof(input: &[u8], output_len: usize) -> Vec<u8> {
    let mut hasher = Hasher::new();
    hasher.update(input);
    let mut reader = hasher.finalize_xof();
    let mut output = vec![0u8; output_len];
    reader.fill(&mut output);
    output
}

/// Generates arbitrary-length output from multiple Blake3 inputs using XOF mode.
///
/// This variant accepts multiple input slices which are concatenated
/// before hashing. Useful when building complex domain-separated inputs.
///
/// # Arguments
/// * `inputs` - Slice of input byte slices
/// * `output_len` - Desired number of output bytes
///
/// # Returns
/// Vector of `output_len` bytes
pub fn blake3_xof_multi(inputs: &[&[u8]], output_len: usize) -> Vec<u8> {
    let mut hasher = Hasher::new();
    for input in inputs {
        hasher.update(input);
    }
    let mut reader = hasher.finalize_xof();
    let mut output = vec![0u8; output_len];
    reader.fill(&mut output);
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_blake3_256() {
        let data = b"hello world";
        let hash = blake3_256(data);
        assert_eq!(hash.0.len(), 32);
        let hash2 = blake3_256(data);
        assert_eq!(hash, hash2);
    }

    #[test]
    fn test_sha3_256() {
        let data = b"hello world";
        let hash = sha3_256(data);
        assert_eq!(hash.0.len(), 32);
        let hash2 = sha3_256(data);
        assert_eq!(hash, hash2);
    }

    #[test]
    fn test_blake3_xof() {
        let input = b"test";
        let output = blake3_xof(input, 64);
        assert_eq!(output.len(), 64);
        let output2 = blake3_xof(input, 64);
        assert_eq!(output, output2);
    }

    #[test]
    fn test_blake3_xof_1mb() {
        let input = b"test";
        let output = blake3_xof(input, 1_048_576);
        assert_eq!(output.len(), 1_048_576);
    }

    #[test]
    fn test_blake3_xof_multi() {
        let inputs: &[&[u8]] = &[b"part1", b"part2", b"part3"];
        let output = blake3_xof_multi(inputs, 64);
        assert_eq!(output.len(), 64);
    }

    #[test]
    fn test_different_inputs_different_hashes() {
        let h1 = blake3_256(b"hello");
        let h2 = blake3_256(b"world");
        assert_ne!(h1, h2);
        let h3 = sha3_256(b"hello");
        let h4 = sha3_256(b"world");
        assert_ne!(h3, h4);
    }
}
