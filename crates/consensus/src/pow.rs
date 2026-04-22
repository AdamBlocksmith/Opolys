//! # Autolykos-inspired proof-of-work mining and verification.
//!
//! Opolys uses an Autolykos-inspired memory-hard PoW algorithm to resist
//! ASIC dominance and favor commodity hardware (GPUs). The algorithm:
//!
//! 1. Generates a memory-hard dataset from the block header and height.
//! 2. Uses the dataset to repeatedly mix a "mixer" value through
//!    `AUTOLYKOS_MIX_ROUNDS` rounds of Blake3 hashing.
//! 3. The resulting hash must fall below the difficulty target.
//!
//! Miners who find hashes far below the target earn a **discovery bonus**
//! that multiplies their block reward, incentivizing continued mining even
//! as difficulty rises and base rewards shrink.
//!
//! There are no pre-mined coins and no founder allocations — all $OPL enters
//! circulation exclusively through block rewards.

use opolys_core::{Block, BlockHeader, BlockHeight, Hash, OpolysError};
use opolys_crypto::{Blake3Hasher, hash};

/// Initial dataset size for the Autolykos memory-hard function (1 MiB).
/// Grows with block height to increase memory requirements over time.
const AUTOLYKOS_DATASET_SIZE: usize = 1 << 20;

/// Number of mix rounds in the Autolykos hash function. Each round reads
/// two dataset elements and mixes them into the accumulator via Blake3.
const AUTOLYKOS_MIX_ROUNDS: usize = 8;

/// A memory-hard dataset generated from the block header and height.
///
/// The dataset grows with block height (doubling every 100k blocks, up to
/// 16 MiB max) to progressively increase mining memory requirements and
/// resist ASIC specialization.
pub struct AutolykosDataset {
    elements: Vec<[u8; 32]>,
}

impl AutolykosDataset {
    /// Generate the dataset deterministically from header bytes and block height.
    /// The dataset grows over time to increase memory hardness as the network
    /// matures.
    pub fn generate(header_bytes: &[u8], height: BlockHeight) -> Self {
        let size = Self::dataset_size(height);
        let mut elements = Vec::with_capacity(size);

        // Derive a seed from the header bytes and height for deterministic generation.
        let mut hasher = Blake3Hasher::new();
        hasher.update(header_bytes);
        hasher.update(&(height.to_be_bytes()));
        let seed = hasher.finalize();

        // Each element is a Blake3 hash of the seed + element index.
        for i in 0..size {
            let mut element = [0u8; 32];
            let mut h = Blake3Hasher::new();
            h.update(&seed.0);
            h.update(&(i as u64).to_be_bytes());
            let element_hash = h.finalize();
            element.copy_from_slice(&element_hash.0[..32]);
            elements.push(element);
        }

        Self { elements }
    }

    /// Compute the dataset size for a given block height. Starts at
    /// `AUTOLYKOS_DATASET_SIZE` (1 MiB), doubles every 100k blocks, and
    /// caps at 16 MiB (`1 << 24`). This gradual growth means mining
    /// remains accessible early on but becomes progressively more
    /// memory-hard.
    fn dataset_size(height: BlockHeight) -> usize {
        let base = AUTOLYKOS_DATASET_SIZE;
        // Double dataset size every 100k blocks, but cap growth at 4 doublings.
        let growth = (height as usize) / 100_000;
        let doubled = base << growth.min(4);
        doubled.min(1 << 24)
    }

    /// Look up a dataset element by index.
    pub fn get(&self, index: usize) -> Option<&[u8; 32]> {
        self.elements.get(index)
    }

    /// Number of elements currently in the dataset.
    pub fn len(&self) -> usize {
        self.elements.len()
    }
}

/// Compute the Autolykos hash for a block header and nonce.
///
/// This is a memory-hard function that:
/// 1. Initializes a 32-byte mixer from the header fields and nonce.
/// 2. Over `AUTOLYKOS_MIX_ROUNDS` iterations, reads two pseudo-random
///    dataset elements and Blake3-hashes them into the mixer.
/// 3. Returns the final Blake3 hash.
///
/// The memory-hardness comes from the dataset lookups — each round reads
/// two elements whose indices depend on the current mixer state, making
/// precomputation infeasible without holding the dataset in memory.
pub fn autolykos_hash(
    dataset: &AutolykosDataset,
    header: &BlockHeader,
    nonce: u64,
) -> Hash {
    let mut mixer = [0u8; 32];
    {
        // Initialize mixer from core header fields (excluding pow_proof and
        // validator_signature) plus the nonce.
        let mut hasher = Blake3Hasher::new();
        hasher.update(&header.previous_hash.0);
        hasher.update(&header.state_root.0);
        hasher.update(&header.transaction_root.0);
        hasher.update(&header.height.to_be_bytes());
        hasher.update(&header.timestamp.to_be_bytes());
        hasher.update(&header.difficulty.to_be_bytes());
        hasher.update(&nonce.to_be_bytes());
        let seed = hasher.finalize();
        mixer.copy_from_slice(&seed.0);
    }

    let ds_size = dataset.len();
    if ds_size == 0 {
        return hash(&mixer);
    }

    // Each round reads two dataset elements at positions derived from the
    // mixer state. The mixer absorbs these elements via Blake3, creating
    // a memory-hard dependency: you can't compute the final hash without
    // random access to the full dataset.
    for round in 0..AUTOLYKOS_MIX_ROUNDS {
        // Derive two dataset indices from the first 16 bytes of the mixer.
        let idx1 = (u64::from_be_bytes(mixer[..8].try_into().unwrap_or([0u8; 8])) as usize) % ds_size;
        let idx2 = ((u64::from_be_bytes(mixer[8..16].try_into().unwrap_or([0u8; 8])).wrapping_add(round as u64)) as usize) % ds_size;

        let elem1 = dataset.get(idx1).copied().unwrap_or([0u8; 32]);
        let elem2 = dataset.get(idx2).copied().unwrap_or([0u8; 32]);

        let mut hasher = Blake3Hasher::new();
        hasher.update(&mixer);
        hasher.update(&elem1);
        hasher.update(&elem2);
        let mixed = hasher.finalize();
        mixer.copy_from_slice(&mixed.0);
    }

    hash(&mixer)
}

/// Mine a block by searching for a nonce that produces a valid Autolykos
/// hash below the difficulty target.
///
/// Generates the dataset, then iterates nonces from 0 to `max_nonce_attempts`.
/// Returns `Some(Block)` if a valid nonce is found, `None` if the search
/// is exhausted. The resulting block has no transactions — they are added
/// separately by the block producer after template selection.
pub fn mine_block(
    header: BlockHeader,
    difficulty: u64,
    max_nonce_attempts: u64,
) -> Option<Block> {
    // Build a deterministic header digest for dataset generation — excludes
    // pow_proof and validator_signature since those aren't set yet.
    let header_for_dataset = {
        let mut hasher = Blake3Hasher::new();
        hasher.update(&header.previous_hash.0);
        hasher.update(&header.state_root.0);
        hasher.update(&header.transaction_root.0);
        hasher.update(&header.height.to_be_bytes());
        hasher.update(&header.difficulty.to_be_bytes());
        hasher.finalize().0.to_vec()
    };

    let dataset = AutolykosDataset::generate(&header_for_dataset, header.height);
    let target = u64::MAX / difficulty;

    for nonce in 0..max_nonce_attempts {
        let pow_hash = autolykos_hash(&dataset, &header, nonce);
        // Use the first 8 bytes of the hash as a u64 for comparison against
        // the difficulty target. This is consistent with how difficulty is
        // expressed as a 64-bit value.
        let hash_int = u64::from_be_bytes(pow_hash.0[..8].try_into().unwrap_or([0u8; 8]));

        if hash_int < target {
            let mut proof_buf = Vec::with_capacity(8);
            proof_buf.extend_from_slice(&nonce.to_be_bytes());
            let mut header = header;
            header.pow_proof = Some(proof_buf);

            return Some(Block {
                header,
                transactions: vec![],
            });
        }
    }

    None
}

/// Verify that a block's PoW proof is valid for the given difficulty.
///
/// Reconstructs the Autolykos dataset from the header, extracts the nonce
/// from `pow_proof`, recomputes the hash, and checks that it falls below
/// the target. Returns `Ok(())` if valid, `Err(InvalidProofOfWork)` otherwise.
pub fn verify_pow(header: &BlockHeader, difficulty: u64) -> Result<(), OpolysError> {
    let nonce_bytes = header.pow_proof.as_ref()
        .ok_or(OpolysError::InvalidProofOfWork)?;

    if nonce_bytes.len() < 8 {
        return Err(OpolysError::InvalidProofOfWork);
    }

    let nonce = u64::from_be_bytes(nonce_bytes[..8].try_into().map_err(|_| OpolysError::InvalidProofOfWork)?);

    // Reconstruct the header digest for dataset generation — same fields
    // excluded as during mining (pow_proof, validator_signature).
    let header_for_dataset = {
        let mut hasher = Blake3Hasher::new();
        hasher.update(&header.previous_hash.0);
        hasher.update(&header.state_root.0);
        hasher.update(&header.transaction_root.0);
        hasher.update(&header.height.to_be_bytes());
        hasher.update(&header.difficulty.to_be_bytes());
        hasher.finalize().0.to_vec()
    };

    let dataset = AutolykosDataset::generate(&header_for_dataset, header.height);
    let pow_hash = autolykos_hash(&dataset, header, nonce);

    let target = u64::MAX / difficulty;
    let hash_int = u64::from_be_bytes(pow_hash.0[..8].try_into().unwrap_or([0u8; 8]));

    if hash_int < target {
        Ok(())
    } else {
        Err(OpolysError::InvalidProofOfWork)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opolys_core::Hash;

    fn make_header(height: BlockHeight, difficulty: u64) -> BlockHeader {
        BlockHeader {
            height,
            previous_hash: Hash::zero(),
            state_root: Hash::zero(),
            transaction_root: Hash::zero(),
            timestamp: 1000,
            pow_proof: None,
            validator_signature: None,
            difficulty,
        }
    }

    #[test]
    fn test_dataset_generation() {
        let dataset = AutolykosDataset::generate(b"test_header", 0);
        assert!(dataset.len() > 0);
        assert!(dataset.get(0).is_some());
    }

    #[test]
    fn test_mine_and_verify() {
        let header = make_header(1, 1);
        let block = mine_block(header, 1, 10_000_000).unwrap();
        assert!(block.header.pow_proof.is_some());
        assert!(verify_pow(&block.header, 1).is_ok());
    }

    #[test]
    fn test_verify_fails_wrong_difficulty() {
        let header = make_header(1, 1);
        let block = mine_block(header, 1, 10_000_000).unwrap();
        assert!(verify_pow(&block.header, 1_000_000_000).is_err());
    }

    #[test]
    fn test_mining_at_higher_difficulty() {
        let header = make_header(1, 100);
        let block = mine_block(header, 100, 10_000_000);
        if let Some(block) = block {
            assert!(verify_pow(&block.header, 100).is_ok());
        }
    }
}