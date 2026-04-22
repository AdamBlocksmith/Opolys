use opolys_core::{Hash, ObjectId};

pub struct Blake3Hasher {
    hasher: blake3::Hasher,
}

impl Blake3Hasher {
    pub fn new() -> Self {
        Self {
            hasher: blake3::Hasher::new(),
        }
    }

    pub fn update(&mut self, data: &[u8]) {
        self.hasher.update(data);
    }

    pub fn finalize(&self) -> Hash {
        let hash = self.hasher.finalize();
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

pub fn hash(data: &[u8]) -> Hash {
    let mut hasher = Blake3Hasher::new();
    hasher.update(data);
    hasher.finalize()
}

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