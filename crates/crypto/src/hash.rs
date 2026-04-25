//! Blake3-256 and SHA3-256 hashing for the Opolys blockchain.
//!
//! Blake3-256 (32 bytes) is the primary hash algorithm used across Opolys for
//! `Hash` and `ObjectId`. SHA3-256 is used for EVO-OMAP finalization and
//! domain-separated commitments where a distinct hash is required.
//!
//! Every `Hash` and `ObjectId` on the network is produced by Blake3, ensuring
//! a uniform, collision-resistant digest that is fast to compute and easy to
//! verify. SHA3-256 provides a separate hash function for cases where a second
//! independent hash is needed (e.g., EVO-OMAP finalization: Blake3 inner with
//! SHA3-256 final).
//!
//! Blake3 XOF (extendable output function) is used for EVO-OMAP's dataset
//! generation and variable-length operations, providing deterministic
//! arbitrary-length output from a single seed.

use opolys_core::{Hash, ObjectId};

/// A streaming Blake3-256 hasher that incrementally absorbs input data.
///
/// Use this when the data to be hashed is large or arrives in chunks. Call
/// [`update`](Self::update) one or more times, then
/// [`finalize`](Self::finalize) to produce a 32-byte [`Hash`].
///
/// # Example
///
/// ```ignore
/// let mut h = Blake3Hasher::new();
/// h.update(b"hello");
/// h.update(b" world");
/// let digest = h.finalize(); // same as hash(b"hello world")
/// ```
pub struct Blake3Hasher {
    hasher: blake3::Hasher,
}

impl Blake3Hasher {
    /// Create a new hasher in its initial (empty-input) state.
    pub fn new() -> Self {
        Self {
            hasher: blake3::Hasher::new(),
        }
    }

    /// Absorb additional bytes into the running hash state.
    ///
    /// May be called zero or more times before [`finalize`](Self::finalize).
    /// Order matters: `update(a); update(b)` produces the same digest as
    /// `update(a.concat(b))`, but a different digest from `update(b); update(a)`.
    pub fn update(&mut self, data: &[u8]) {
        self.hasher.update(data);
    }

    /// Finalize the hash and return a 32-byte Blake3-256 [`Hash`].
    ///
    /// This consumes the hasher's internal state. Calling `finalize` again on
    /// the same hasher (without further `update` calls) will return the same
    /// value, because the underlying `blake3::Hasher` is not mutably consumed.
    pub fn finalize(&self) -> Hash {
        let hash = self.hasher.finalize();
        // Blake3 outputs exactly 32 bytes by default — the Opolys standard.
        let mut result = [0u8; 32];
        result.copy_from_slice(hash.as_bytes());
        Hash::from_bytes(result)
    }
}

impl Default for Blake3Hasher {
    fn default() -> Self {
        Self::new()
    }
}

/// Compute the Blake3-256 hash of arbitrary data in a single call.
///
/// This is the canonical one-shot hash function used throughout Opolys for
/// producing [`Hash`] values (e.g. block hashes, transaction IDs, Merkle
/// leaves). The output is always exactly 32 bytes.
///
/// For streaming / incremental hashing, see [`Blake3Hasher`].
pub fn hash(data: &[u8]) -> Hash {
    let mut hasher = Blake3Hasher::new();
    hasher.update(data);
    hasher.finalize()
}

/// Compute a Blake3-256 hash of the input and wrap it as an [`ObjectId`].
///
/// On the Opolys network, every on-chain object (transaction, block header,
/// etc.) is identified by its Blake3-256 digest wrapped in `ObjectId`. This
/// function is the standard way to derive such an identifier from raw bytes.
pub fn hash_to_object_id(data: &[u8]) -> ObjectId {
    ObjectId(hash(data))
}

/// Compute SHA3-256 of arbitrary data.
///
/// Used for EVO-OMAP finalization (Blake3 inner, SHA3-256 outer) and
/// domain-separated commitments where a second independent hash is required.
/// Produces a 32-byte digest, same length as Blake3-256, but with different
/// internal structure — ensuring no accidental collisions between Blake3 and
/// SHA3 outputs.
pub fn sha3_256(data: &[u8]) -> Hash {
    use sha3::Digest;
    let mut hasher = sha3::Sha3_256::new();
    hasher.update(data);
    let result = hasher.finalize();
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&result[..32]);
    Hash::from_bytes(arr)
}

/// Compute Blake3 XOF (extendable output) to produce arbitrary-length output.
///
/// Used by EVO-OMAP for dataset generation and other variable-length operations.
/// Given a seed and a desired output length, produces deterministic bytes.
/// Unlike `hash()`, which always produces 32 bytes, XOF can produce any length.
///
/// Uses Blake3's native XOF mode via `Hasher::new_xof()` + `XofReader`,
/// which supports arbitrary output lengths from a single seed.
pub fn blake3_xof(seed: &[u8], output_len: usize) -> Vec<u8> {
    let mut output = vec![0u8; output_len];
    let mut hasher = blake3::Hasher::new();
    hasher.update(seed);
    let mut reader = hasher.finalize_xof();
    reader.fill(&mut output);
    output
}

/// Compute Blake3 XOF with multiple input slices concatenated as seed.
///
/// Convenience function for EVO-OMAP operations that need to hash multiple
/// domain-separated inputs before extending to the desired output length.
pub fn blake3_xof_multi(inputs: &[&[u8]], output_len: usize) -> Vec<u8> {
    let mut hasher = blake3::Hasher::new();
    for input in inputs {
        hasher.update(input);
    }
    let mut output = vec![0u8; output_len];
    let mut reader = hasher.finalize_xof();
    reader.fill(&mut output);
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_deterministic() {
        let h1 = hash(b"test data");
        let h2 = hash(b"test data");
        assert_eq!(h1, h2);
    }

    #[test]
    fn hash_different_inputs() {
        let h1 = hash(b"data1");
        let h2 = hash(b"data2");
        assert_ne!(h1, h2);
    }

    #[test]
    fn hash_to_object_id_produces_valid_id() {
        let id = hash_to_object_id(b"test");
        assert_ne!(id.0, Hash::zero());
    }

    #[test]
    fn blake3_hasher_streaming() {
        let mut h1 = Blake3Hasher::new();
        h1.update(b"hello");
        h1.update(b" world");
        let r1 = h1.finalize();

        let h2 = hash(b"hello world");
        assert_eq!(r1, h2);
    }

    #[test]
    fn hash_is_32_bytes() {
        let h = hash(b"test");
        assert_eq!(h.as_bytes().len(), 32);
        assert_eq!(h.to_hex().len(), 64);
    }
}