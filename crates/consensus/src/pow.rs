//! # EVO-OMAP proof-of-work mining and verification.
//!
//! Opolys uses EVO-OMAP (EVOlutionary Oriented Memory-hard Algorithm for
//! Proof-of-work) as its mining algorithm. EVO-OMAP is execution-based —
//! miners must execute a deterministic program that reads and writes a 256 MiB
//! dataset, forcing specialized hardware to include large SRAM.
//!
//! Key properties:
//! - 256 MiB mutable dataset, regenerated per epoch (960 blocks)
//! - Blake3 inner hashing with SHA3-256 finalization
//! - 4-way data-dependent branching resists GPU warp efficiency
//! - 8 superscalar instructions per step with memory-dependent operands
//! - Light verification via on-demand node reconstruction
//!
//! There are no pre-mined coins and no founder allocations — all $OPL enters
//! circulation exclusively through block rewards.

use evo_omap::DatasetCache;
use opolys_core::{Block, BlockHeader, EPOCH, MAINNET_CHAIN_ID, OpolysError};

/// EVO-OMAP dataset cache for efficient epoch-based mining.
///
/// The dataset is regenerated when the epoch changes (every EPOCH blocks).
/// This cache avoids regenerating the 256 MiB dataset within an epoch —
/// a ~7.5 second cost that would otherwise be paid per nonce attempt.
pub struct PowContext {
    cache: DatasetCache,
    last_epoch: Option<u64>,
}

impl PowContext {
    /// Create a new mining context with an empty dataset cache.
    pub fn new() -> Self {
        PowContext {
            cache: DatasetCache::new(),
            last_epoch: None,
        }
    }

    /// Get or regenerate the dataset for the given block header.
    ///
    /// The dataset seed is bound to Opolys mainnet identity and the parent hash,
    /// so future work cannot be precomputed without knowing real chain history.
    pub fn get_dataset(&mut self, header: &BlockHeader) -> &evo_omap::Dataset {
        let height = header.height;
        let epoch = height / EPOCH;
        let seed_material = epoch_seed_material(header);
        if self.last_epoch != Some(epoch) {
            self.cache.get_dataset_with_epoch_length_and_seed_material(
                height,
                EPOCH,
                &seed_material,
            );
            self.last_epoch = Some(epoch);
        }
        self.cache
            .get_dataset_with_epoch_length_and_seed_material(height, EPOCH, &seed_material)
    }

    /// Mine a block using EVO-OMAP with parallel nonce search.
    ///
    /// Uses all available CPU cores via rayon. Returns `Some(Block)` if a
    /// valid nonce is found within `max_attempts`, `None` otherwise.
    pub fn mine_parallel(
        &mut self,
        header: BlockHeader,
        difficulty: u64,
        max_attempts: u64,
        num_threads: usize,
    ) -> Option<Block> {
        let header_bytes = serialize_header_for_pow(&header);
        let height = header.height;

        // Ensure dataset is available for this epoch
        let seed_material = epoch_seed_material(&header);
        self.get_dataset(&header);

        let (nonce_result, _attempts) = evo_omap::mine_parallel_with_epoch_length_and_seed_material(
            &header_bytes,
            height,
            difficulty,
            max_attempts,
            num_threads,
            EPOCH,
            &seed_material,
        );
        let nonce = match nonce_result {
            Some(n) => n,
            None => return None,
        };

        let mut proof_buf = Vec::with_capacity(8);
        proof_buf.extend_from_slice(&nonce.to_be_bytes());

        let mut header = header;
        header.pow_proof = Some(proof_buf);

        Some(Block {
            header,
            transactions: vec![],
            slash_evidence: vec![],
            genesis_ceremony: None,
        })
    }

    /// Mine a block using EVO-OMAP single-threaded (for testing/fallback).
    pub fn mine_single(
        &mut self,
        header: BlockHeader,
        difficulty: u64,
        max_attempts: u64,
    ) -> Option<Block> {
        let header_bytes = serialize_header_for_pow(&header);
        let height = header.height;

        let seed_material = epoch_seed_material(&header);
        self.get_dataset(&header);

        let (nonce_result, _attempts) = evo_omap::mine_with_epoch_length_and_seed_material(
            &header_bytes,
            height,
            difficulty,
            max_attempts,
            EPOCH,
            &seed_material,
        );
        let nonce = match nonce_result {
            Some(n) => n,
            None => return None,
        };

        let mut proof_buf = Vec::with_capacity(8);
        proof_buf.extend_from_slice(&nonce.to_be_bytes());

        let mut header = header;
        header.pow_proof = Some(proof_buf);

        Some(Block {
            header,
            transactions: vec![],
            slash_evidence: vec![],
            genesis_ceremony: None,
        })
    }
}

impl Default for PowContext {
    fn default() -> Self {
        Self::new()
    }
}

/// Serialize the block header fields for EVO-OMAP mining.
///
/// Includes all fields except `pow_proof` and `refiner_signature`,
/// which are set after mining. Also includes `version` and `suggested_fee`
/// to bind the PoW to the complete header state.
///
/// Opolys serializes block-header integers in big-endian network order at this
/// boundary. EVO-OMAP then interprets its internal VM words in little-endian
/// order by specification; cross-client implementations must preserve both.
pub fn serialize_header_for_pow(header: &BlockHeader) -> Vec<u8> {
    let mut buf = Vec::with_capacity(256);
    buf.extend_from_slice(&header.version.to_be_bytes());
    buf.extend_from_slice(&header.height.to_be_bytes());
    buf.extend_from_slice(&header.previous_hash.0);
    buf.extend_from_slice(&header.state_root.0);
    buf.extend_from_slice(&header.transaction_root.0);
    buf.extend_from_slice(&header.timestamp.to_be_bytes());
    buf.extend_from_slice(&header.difficulty.to_be_bytes());
    buf.extend_from_slice(&header.suggested_fee.to_be_bytes());
    if let Some(ref ext_root) = header.extension_root {
        buf.extend_from_slice(&ext_root.0);
    }
    buf
}

/// Build the Opolys-specific EVO-OMAP epoch seed material.
///
/// This binds the memory dataset to both mainnet identity and the current
/// parent hash, preventing durable future-dataset precomputation.
pub fn epoch_seed_material(header: &BlockHeader) -> Vec<u8> {
    let mut material = Vec::with_capacity(40);
    material.extend_from_slice(&MAINNET_CHAIN_ID.to_be_bytes());
    material.extend_from_slice(&header.previous_hash.0);
    material
}

/// Verify a block's PoW using light verification (on-demand node reconstruction).
///
/// Requires no pre-generated cache — nodes reconstruct dataset nodes as needed.
/// Trades computation for memory (~7.5s verification time, no 256 MiB cache).
/// Returns `Ok(())` if valid, `Err(InvalidProofOfWork)` otherwise.
pub fn verify_pow_light(header: &BlockHeader, difficulty: u64) -> Result<(), OpolysError> {
    if difficulty == 0 {
        return Ok(());
    }

    let nonce_bytes = header
        .pow_proof
        .as_ref()
        .ok_or(OpolysError::InvalidProofOfWork)?;

    if nonce_bytes.len() < 8 {
        return Err(OpolysError::InvalidProofOfWork);
    }

    let nonce = u64::from_be_bytes(
        nonce_bytes[..8]
            .try_into()
            .map_err(|_| OpolysError::InvalidProofOfWork)?,
    );

    let header_bytes = serialize_header_for_pow(header);
    let seed_material = epoch_seed_material(header);
    evo_omap::verify_light_with_epoch_length_and_seed_material(
        &header_bytes,
        header.height,
        nonce,
        difficulty,
        EPOCH,
        &seed_material,
    )
    .then_some(())
    .ok_or(OpolysError::InvalidProofOfWork)
}

/// Compute the PoW hash value as a u64 for vein yield calculation.
///
/// Uses light verification (on-demand node reconstruction), so no 256 MiB
/// dataset is required. Returns `None` if the header has no PoW proof.
pub fn compute_pow_hash_value(header: &BlockHeader) -> Option<u64> {
    let nonce_bytes = header.pow_proof.as_ref()?;
    if nonce_bytes.len() < 8 {
        return None;
    }
    let nonce = u64::from_be_bytes(nonce_bytes[..8].try_into().ok()?);
    let header_bytes = serialize_header_for_pow(header);
    let seed_material = epoch_seed_material(header);
    let epoch_seed = evo_omap::compute_epoch_seed_with_epoch_length_and_seed_material(
        header.height,
        EPOCH,
        &seed_material,
    );
    let mut dataset = evo_omap::LightDataset::new(&epoch_seed);
    let hash = evo_omap::evo_omap_hash_light(&mut dataset, &header_bytes, header.height, nonce);
    Some(u64::from_be_bytes(
        hash.0[..8].try_into().unwrap_or([0u8; 8]),
    ))
}

/// Compute a deterministic EVO-OMAP hash for the memory-fingerprinting challenge protocol.
///
/// Uses light verification (on-demand node reconstruction) over synthetic input bytes
/// derived from (height, nonce). Answering correctly requires the 256 MiB dataset.
pub fn compute_challenge_hash(height: u64, nonce: u64) -> u64 {
    let epoch_seed = evo_omap::compute_epoch_seed_with_epoch_length(height, EPOCH);
    let mut dataset = evo_omap::LightDataset::new(&epoch_seed);
    let mut input = [0u8; 16];
    input[..8].copy_from_slice(&height.to_be_bytes());
    input[8..].copy_from_slice(&nonce.to_be_bytes());
    let hash = evo_omap::evo_omap_hash_light(&mut dataset, &input, height, nonce);
    u64::from_be_bytes(hash.0[..8].try_into().unwrap_or([0u8; 8]))
}

/// Convenience function: mine a block without persistent caching.
///
/// Creates a temporary `PowContext` for one-off mining. For production mining,
/// use `PowContext::mine_parallel` instead to avoid regenerating the dataset
/// every call.
pub fn mine_block(header: BlockHeader, difficulty: u64, max_attempts: u64) -> Option<Block> {
    let mut ctx = PowContext::new();
    ctx.mine_parallel(header, difficulty, max_attempts, 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use opolys_core::{BLOCK_VERSION, Hash, ObjectId};

    fn make_header(height: u64, difficulty: u64) -> BlockHeader {
        BlockHeader {
            version: BLOCK_VERSION,
            height,
            previous_hash: Hash::zero(),
            state_root: Hash::zero(),
            transaction_root: Hash::zero(),
            timestamp: 1000,
            difficulty,
            suggested_fee: 1,
            extension_root: None,
            producer: ObjectId(Hash::zero()),
            pow_proof: None,
            refiner_signature: None,
        }
    }

    #[test]
    fn test_pow_context_creation() {
        let ctx = PowContext::new();
        assert_eq!(ctx.last_epoch, None);
    }

    #[test]
    fn opolys_pow_epoch_seed_rotates_at_960() {
        let seed_0 = evo_omap::compute_epoch_seed_with_epoch_length(0, EPOCH);
        let seed_959 = evo_omap::compute_epoch_seed_with_epoch_length(EPOCH - 1, EPOCH);
        let seed_960 = evo_omap::compute_epoch_seed_with_epoch_length(EPOCH, EPOCH);
        let default_seed_960 = evo_omap::compute_epoch_seed(EPOCH);

        assert_eq!(seed_0, seed_959);
        assert_ne!(seed_0, seed_960);
        assert_eq!(default_seed_960, evo_omap::compute_epoch_seed(0));
        assert_ne!(seed_960, default_seed_960);
    }

    #[test]
    fn opolys_pow_epoch_seed_is_bound_to_parent_hash() {
        let header_a = BlockHeader {
            previous_hash: Hash::from_bytes([1u8; 32]),
            ..make_header(EPOCH, 1)
        };
        let header_b = BlockHeader {
            previous_hash: Hash::from_bytes([2u8; 32]),
            ..make_header(EPOCH, 1)
        };

        let seed_a = evo_omap::compute_epoch_seed_with_epoch_length_and_seed_material(
            header_a.height,
            EPOCH,
            &epoch_seed_material(&header_a),
        );
        let seed_b = evo_omap::compute_epoch_seed_with_epoch_length_and_seed_material(
            header_b.height,
            EPOCH,
            &epoch_seed_material(&header_b),
        );
        let unbound_seed = evo_omap::compute_epoch_seed_with_epoch_length(header_a.height, EPOCH);

        assert_ne!(seed_a, seed_b);
        assert_ne!(seed_a, unbound_seed);
    }

    #[test]
    fn test_header_serialization_deterministic() {
        let header = make_header(42, 100);
        let bytes1 = serialize_header_for_pow(&header);
        let bytes2 = serialize_header_for_pow(&header);
        assert_eq!(bytes1, bytes2);
    }

    #[test]
    fn test_header_serialization_includes_version() {
        let h1 = make_header(1, 1);
        let h2 = BlockHeader {
            version: 2,
            ..h1.clone()
        };
        let b1 = serialize_header_for_pow(&h1);
        let b2 = serialize_header_for_pow(&h2);
        assert_ne!(b1, b2);
    }

    #[test]
    fn test_header_serialization_uses_big_endian_boundary() {
        let header = BlockHeader {
            version: 0x01020304,
            height: 0x05060708090a0b0c,
            timestamp: 0x0d0e0f1011121314,
            difficulty: 0x15161718191a1b1c,
            suggested_fee: 0x1d1e1f2021222324,
            ..make_header(1, 1)
        };

        let bytes = serialize_header_for_pow(&header);

        assert_eq!(&bytes[0..4], &0x01020304u32.to_be_bytes());
        assert_eq!(&bytes[4..12], &0x05060708090a0b0cu64.to_be_bytes());
        assert_eq!(&bytes[108..116], &0x0d0e0f1011121314u64.to_be_bytes());
        assert_eq!(&bytes[116..124], &0x15161718191a1b1cu64.to_be_bytes());
        assert_eq!(&bytes[124..132], &0x1d1e1f2021222324u64.to_be_bytes());
    }

    #[test]
    fn test_header_serialization_includes_suggested_fee() {
        let h1 = make_header(1, 1);
        let h2 = BlockHeader {
            suggested_fee: 999,
            ..h1.clone()
        };
        let b1 = serialize_header_for_pow(&h1);
        let b2 = serialize_header_for_pow(&h2);
        assert_ne!(b1, b2);
    }

    #[test]
    fn test_verify_pow_rejects_missing_proof() {
        let header = make_header(1, 1);
        let result = verify_pow_light(&header, 1);
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_pow_rejects_short_proof() {
        let mut header = make_header(1, 1);
        header.pow_proof = Some(vec![0u8; 4]);
        let result = verify_pow_light(&header, 1);
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_pow_zero_difficulty() {
        let mut header = make_header(1, 0);
        header.pow_proof = None;
        let result = verify_pow_light(&header, 0);
        assert!(result.is_ok());
    }
}
