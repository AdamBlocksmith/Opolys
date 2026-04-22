//! Blake3-256 hashing for the Opolys blockchain.
//!
//! Blake3-256 (32 bytes) is the sole hash algorithm used across Opolys. Every
//! `Hash` and `ObjectId` on the network is produced by Blake3, ensuring a
//! uniform, collision-resistant digest that is fast to compute and easy to
//! verify. No other hash functions are used — this single-algorithm discipline
//! keeps the protocol simple and auditable.

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