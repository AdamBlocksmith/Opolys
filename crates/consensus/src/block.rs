//! # Block hashing, transaction roots, and display formatting.
//!
//! Opolys blocks are linked by Blake3-256 hashes of their headers. Every header
//! field — except `pow_proof` and `refiner_signature`, which are set after
//! the block is produced — is hashed in a fixed order for deterministic
//! identification. Transaction integrity is guaranteed by a streaming Merkle-
//! like root over each transaction's ID, fee, and nonce.
//!
//! Fees included in blocks are routed by production kind: mined-block fees are
//! burned, while Proof-of-Refinement block fees pay the selected refiner
//! producer.

use opolys_core::{
    BLOCK_TARGET_TIME_SECS, BLOCK_VERSION, Block, BlockAttestation, BlockHeader,
    DoubleSignEvidence, FLAKES_PER_OPL, GenesisCeremonyData, Hash, MAX_ATTESTATIONS_PER_BLOCK,
    MAX_BLOCK_SIZE_BYTES, MAX_FUTURE_BLOCK_TIME_SECS, MAX_TRANSACTIONS_PER_BLOCK,
    MAX_TX_DATA_SIZE_BYTES, OpolysError,
};

/// Maximum slash evidence entries allowed per block.
/// Prevents DoS via unbounded ed25519 verification under the write lock.
pub const MAX_SLASH_EVIDENCE_PER_BLOCK: usize = 10;
use borsh::{BorshDeserialize, BorshSerialize};
use opolys_crypto::{
    Blake3Hasher, DOMAIN_ATTESTATION_ROOT, DOMAIN_BLOCK_HASH, DOMAIN_EVIDENCE_ROOT,
    DOMAIN_GENESIS_CEREMONY_HASH, DOMAIN_TX_ROOT,
};
use serde::{Deserialize, Serialize};

/// Minimum allowed wall-clock progress per non-genesis block.
///
/// This is derived from the existing target interval rather than a separate
/// economic constant. Blocks may still arrive faster than target, allowing
/// difficulty to rise naturally, but not fast enough for a miner to compress a
/// whole epoch into fake one-second timestamps.
pub fn minimum_block_timestamp_delta_secs() -> u64 {
    (BLOCK_TARGET_TIME_SECS / 2).max(1)
}

/// Metadata extracted from a block for indexing and querying.
///
/// Captures the header, transaction count, and total ordinary fees declared in
/// the block. The node decides whether those fees burn or pay a refiner based
/// on the block production kind.
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct BlockInfo {
    /// The block header containing height, hashes, timestamp, and difficulty.
    pub header: BlockHeader,
    /// Number of transactions included in this block.
    pub transaction_count: u32,
    /// Sum of all ordinary transaction fees in this block.
    pub total_transaction_fees: u64,
}

impl BlockInfo {
    /// Derive `BlockInfo` from a complete block by summing all transaction fees.
    pub fn from_block(block: &Block) -> Self {
        let total_transaction_fees: u64 = block.transactions.iter().map(|tx| tx.fee).sum();
        Self {
            header: block.header.clone(),
            transaction_count: block.transactions.len() as u32,
            total_transaction_fees,
        }
    }
}

/// Whether a block has been confirmed by the network.
///
/// In Opolys, transaction finality is immediate once a block is confirmed —
/// there is no reversal window. `Finalized` represents the strongest guarantee;
/// `Orphaned` marks blocks that were on a discarded fork.
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize, Serialize, Deserialize, PartialEq, Eq)]
pub enum BlockStatus {
    /// Block has been received but not yet confirmed.
    Pending,
    /// Block has been confirmed by the network.
    Confirmed,
    /// Block is finalized — irreversible under Opolys consensus rules.
    Finalized,
    /// Block is on a discarded fork and is no longer canonical.
    Orphaned,
}

/// Compute the Blake3-256 hash of a block header.
///
/// This produces a deterministic, 32-byte hash from all header fields,
/// linking each block to its predecessor via `previous_hash`. The genesis
/// block uses `Hash::zero()` as its `previous_hash`.
///
/// Note: `pow_proof` and `refiner_signature` are intentionally excluded
/// because they are populated after mining — the proof must satisfy the hash,
/// not the other way around. The `version`, `suggested_fee`, and
/// `evidence_root`, `attestation_root`, `genesis_ceremony_hash`, and
/// `extension_root` fields are included to bind the PoW to the complete
/// state-mutating block body.
pub fn compute_block_hash(header: &BlockHeader) -> Hash {
    let mut hasher = Blake3Hasher::new();
    hasher.update(DOMAIN_BLOCK_HASH);
    // Hash every field of the header in a fixed order for determinism.
    // pow_proof and refiner_signature are excluded because:
    // - pow_proof is set AFTER mining (the proof must satisfy the hash, not vice versa)
    // - refiner_signature (ed25519) is appended after block producer selection
    // producer IS included because it identifies who earns the block reward.
    hasher.update(&header.version.to_be_bytes());
    hasher.update(&header.height.to_be_bytes());
    hasher.update(&header.previous_hash.0);
    hasher.update(header.producer.as_bytes());
    hasher.update(&header.state_root.0);
    hasher.update(&header.transaction_root.0);
    hasher.update(&header.evidence_root.0);
    hasher.update(&header.attestation_root.0);
    hasher.update(&header.genesis_ceremony_hash.0);
    hasher.update(&header.timestamp.to_be_bytes());
    hasher.update(&header.difficulty.to_be_bytes());
    hasher.update(&header.suggested_fee.to_be_bytes());
    if let Some(ref ext_root) = header.extension_root {
        hasher.update(ext_root.as_bytes());
    }
    hasher.finalize()
}

/// Compute the Merkle-like root hash of all transactions in a block.
///
/// Uses a streaming Blake3 hash over each transaction's ID, fee, and nonce —
/// enough to uniquely identify each transaction and detect any reordering.
/// This is not a binary Merkle tree but a linear commitment that still binds
/// the transaction set deterministically.
pub fn compute_transaction_root(transactions: &[opolys_core::Transaction]) -> Hash {
    let mut hasher = Blake3Hasher::new();
    hasher.update(DOMAIN_TX_ROOT);
    for tx in transactions {
        // Each transaction is committed by its unique ID, fee, and nonce.
        hasher.update(&tx.tx_id.0.0);
        hasher.update(&tx.fee.to_be_bytes());
        hasher.update(&tx.nonce.to_be_bytes());
    }
    hasher.finalize()
}

/// Compute the canonical root of all slash evidence entries in a block.
pub fn compute_evidence_root(evidence: &[DoubleSignEvidence]) -> Result<Hash, OpolysError> {
    let bytes = borsh::to_vec(evidence).map_err(|e| {
        OpolysError::SerializationError(format!("Double-sign evidence serialization failed: {}", e))
    })?;
    Ok(opolys_crypto::hash_with_domain(
        DOMAIN_EVIDENCE_ROOT,
        &bytes,
    ))
}

/// Compute the canonical root of all refiner attestations in a block.
pub fn compute_attestation_root(attestations: &[BlockAttestation]) -> Result<Hash, OpolysError> {
    let bytes = borsh::to_vec(attestations).map_err(|e| {
        OpolysError::SerializationError(format!("Block attestation serialization failed: {}", e))
    })?;
    Ok(opolys_crypto::hash_with_domain(
        DOMAIN_ATTESTATION_ROOT,
        &bytes,
    ))
}

/// Compute the canonical commitment to the optional genesis ceremony payload.
pub fn compute_genesis_ceremony_hash(
    ceremony: &Option<GenesisCeremonyData>,
) -> Result<Hash, OpolysError> {
    let bytes = borsh::to_vec(ceremony).map_err(|e| {
        OpolysError::SerializationError(format!("Genesis ceremony serialization failed: {}", e))
    })?;
    Ok(opolys_crypto::hash_with_domain(
        DOMAIN_GENESIS_CEREMONY_HASH,
        &bytes,
    ))
}

/// Fill all block-body roots into a header before hashing, mining, signing, or validation.
pub fn set_body_roots(block: &mut Block) -> Result<(), OpolysError> {
    block.header.transaction_root = compute_transaction_root(&block.transactions);
    block.header.evidence_root = compute_evidence_root(&block.slash_evidence)?;
    block.header.attestation_root = compute_attestation_root(&block.attestations)?;
    block.header.genesis_ceremony_hash = compute_genesis_ceremony_hash(&block.genesis_ceremony)?;
    Ok(())
}

/// Format a flake amount as a human-readable OPL string with 6 decimal places.
///
/// One OPL equals `FLAKES_PER_OPL` (1,000,000) flakes. This is a pure
/// display function with no rounding logic — it simply divides and pads.
pub fn format_opl(flakes: u64) -> String {
    let whole = flakes / FLAKES_PER_OPL;
    let frac = flakes % FLAKES_PER_OPL;
    format!("{}.{:06} OPL", whole, frac)
}

/// Comprehensive validation of a block before it is applied to the chain.
///
/// Checks every invariant that a well-formed block must satisfy:
///
/// 1. **Version**: Must match `BLOCK_VERSION` (currently 4).
/// 2. **Height**: Must be exactly `expected_height` (parent height + 1).
/// 3. **Previous hash**: Must match the parent block's hash (or `Hash::zero()` for genesis).
/// 4. **Timestamp**: Must advance by a target-derived minimum from
///    `parent_timestamp` and stay within `MAX_FUTURE_BLOCK_TIME_SECS` of the
///    current wall clock.
/// 5. **Difficulty**: Must match `expected_difficulty`.
/// 6. **Transaction count**: Must not exceed `MAX_TRANSACTIONS_PER_BLOCK`.
/// 7. **Block size**: Must not exceed `MAX_BLOCK_SIZE_BYTES`.
/// 8. **Body roots**: Must match transactions, slash evidence, attestations,
///    and genesis ceremony data.
/// 9. **No duplicate transactions**: Each `tx_id` must be unique within the block.
/// 10. **Transaction data size**: Each `tx.data` must not exceed `MAX_TX_DATA_SIZE_BYTES`.
/// 11. **Fee minimum**: Each transaction fee must be at least `MIN_FEE`.
/// 12. **PoW proof**: For PoW blocks, the proof must satisfy the difficulty target.
///
/// Returns `Ok(())` if the block is valid, or `Err(OpolysError::BlockValidationFailed)`
/// with a descriptive message if validation fails.
pub fn validate_block(
    block: &Block,
    expected_height: u64,
    parent_hash: &Hash,
    parent_timestamp: u64,
    expected_difficulty: u64,
    now_secs: u64,
) -> Result<(), OpolysError> {
    // 1. Version check
    if block.header.version != BLOCK_VERSION {
        return Err(OpolysError::BlockValidationFailed(format!(
            "Block version mismatch: expected {}, got {}",
            BLOCK_VERSION, block.header.version
        )));
    }

    // 2. Height check
    if block.header.height != expected_height {
        return Err(OpolysError::BlockValidationFailed(format!(
            "Block height mismatch: expected {}, got {}",
            expected_height, block.header.height
        )));
    }

    // 3. Previous hash check
    // Genesis block has previous_hash = Hash::zero()
    if &block.header.previous_hash != parent_hash {
        return Err(OpolysError::BlockValidationFailed(format!(
            "Block previous_hash mismatch: expected {}, got {}",
            parent_hash.to_hex(),
            block.header.previous_hash.to_hex()
        )));
    }

    // 4. Timestamp check: must advance by a target-derived minimum and not be too far in the future.
    if expected_height > 0 {
        let min_timestamp = parent_timestamp.saturating_add(minimum_block_timestamp_delta_secs());
        if block.header.timestamp < min_timestamp {
            return Err(OpolysError::BlockValidationFailed(format!(
                "Block timestamp {} is too compressed: minimum allowed is {}",
                block.header.timestamp, min_timestamp
            )));
        }
    }
    if block.header.timestamp > now_secs.saturating_add(MAX_FUTURE_BLOCK_TIME_SECS) {
        return Err(OpolysError::BlockValidationFailed(format!(
            "Block timestamp {} is too far in the future (max allowed: {})",
            block.header.timestamp,
            now_secs.saturating_add(MAX_FUTURE_BLOCK_TIME_SECS)
        )));
    }

    // 5. Difficulty check
    if block.header.difficulty != expected_difficulty {
        return Err(OpolysError::BlockValidationFailed(format!(
            "Block difficulty mismatch: expected {}, got {}",
            expected_difficulty, block.header.difficulty
        )));
    }

    // 6. Transaction count check
    if block.transactions.len() > MAX_TRANSACTIONS_PER_BLOCK {
        return Err(OpolysError::BlockValidationFailed(format!(
            "Too many transactions: {} > {}",
            block.transactions.len(),
            MAX_TRANSACTIONS_PER_BLOCK
        )));
    }

    // 6b. Slash evidence count check — cap before entering ed25519 verification
    if block.slash_evidence.len() > MAX_SLASH_EVIDENCE_PER_BLOCK {
        return Err(OpolysError::BlockValidationFailed(format!(
            "Too many slash evidence entries: {} > {}",
            block.slash_evidence.len(),
            MAX_SLASH_EVIDENCE_PER_BLOCK
        )));
    }

    // 6c. Attestation count check — signature verification is enabled in Pass 2.
    if block.attestations.len() > MAX_ATTESTATIONS_PER_BLOCK {
        return Err(OpolysError::BlockValidationFailed(format!(
            "Too many attestations: {} > {}",
            block.attestations.len(),
            MAX_ATTESTATIONS_PER_BLOCK
        )));
    }

    // 7. Block size check
    let block_size = borsh::to_vec(block).map(|v| v.len()).unwrap_or(usize::MAX);
    if block_size > MAX_BLOCK_SIZE_BYTES {
        return Err(OpolysError::BlockValidationFailed(format!(
            "Block too large: {} bytes > {} bytes",
            block_size, MAX_BLOCK_SIZE_BYTES
        )));
    }

    // 8. Transaction root check
    let computed_root = compute_transaction_root(&block.transactions);
    if block.header.transaction_root != computed_root {
        return Err(OpolysError::BlockValidationFailed(format!(
            "Transaction root mismatch: expected {}, got {}",
            computed_root.to_hex(),
            block.header.transaction_root.to_hex()
        )));
    }

    let computed_evidence_root = compute_evidence_root(&block.slash_evidence)?;
    if block.header.evidence_root != computed_evidence_root {
        return Err(OpolysError::BlockValidationFailed(format!(
            "Evidence root mismatch: expected {}, got {}",
            computed_evidence_root.to_hex(),
            block.header.evidence_root.to_hex()
        )));
    }

    let computed_attestation_root = compute_attestation_root(&block.attestations)?;
    if block.header.attestation_root != computed_attestation_root {
        return Err(OpolysError::BlockValidationFailed(format!(
            "Attestation root mismatch: expected {}, got {}",
            computed_attestation_root.to_hex(),
            block.header.attestation_root.to_hex()
        )));
    }

    let computed_genesis_ceremony_hash = compute_genesis_ceremony_hash(&block.genesis_ceremony)?;
    if block.header.genesis_ceremony_hash != computed_genesis_ceremony_hash {
        return Err(OpolysError::BlockValidationFailed(format!(
            "Genesis ceremony hash mismatch: expected {}, got {}",
            computed_genesis_ceremony_hash.to_hex(),
            block.header.genesis_ceremony_hash.to_hex()
        )));
    }

    // 9. Duplicate transaction check
    let mut seen_tx_ids = std::collections::HashSet::new();
    for tx in &block.transactions {
        if !seen_tx_ids.insert(&tx.tx_id) {
            return Err(OpolysError::BlockValidationFailed(format!(
                "Duplicate transaction {} in block {}",
                tx.tx_id.to_hex(),
                block.header.height
            )));
        }
    }

    // 10. Transaction data size check
    for tx in &block.transactions {
        if tx.data.len() > MAX_TX_DATA_SIZE_BYTES {
            return Err(OpolysError::BlockValidationFailed(format!(
                "Transaction {} data too large: {} bytes > {} bytes",
                tx.tx_id.to_hex(),
                tx.data.len(),
                MAX_TX_DATA_SIZE_BYTES
            )));
        }
    }

    // 11. Fee minimum check
    for tx in &block.transactions {
        if tx.fee < opolys_core::MIN_FEE {
            return Err(OpolysError::BlockValidationFailed(format!(
                "Transaction {} fee below minimum: {} < {}",
                tx.tx_id.to_hex(),
                tx.fee,
                opolys_core::MIN_FEE
            )));
        }
    }

    // 12. Block proof check:
    // - PoW blocks: verify the EVO-OMAP proof-of-work
    // - Refiner blocks: verify the refiner's ed25519 signature over the block hash
    //   The producer's public key must be stored on-chain in the AccountStore.
    // - Genesis block (height 0): skip both
    if expected_height > 0 {
        let production_kind = block.header.production_kind().ok_or_else(|| {
            OpolysError::BlockValidationFailed(
                "Block must have exactly one production proof".to_string(),
            )
        })?;
        if block.header.producer.0.is_zero() {
            return Err(OpolysError::BlockValidationFailed(
                "Block producer must be a non-zero ObjectId".to_string(),
            ));
        }

        if production_kind == opolys_core::BlockProductionKind::Mined {
            // PoW block — verify the proof-of-work
            crate::pow::verify_pow_light(&block.header, block.header.difficulty)?;
        } else if production_kind == opolys_core::BlockProductionKind::Refined {
            // Refiner block — verify the refiner's ed25519 signature
            // 1. The signature must be exactly 64 bytes (ed25519)
            let sig = block.header.refiner_signature.as_ref().unwrap();
            if sig.len() != 64 {
                return Err(OpolysError::BlockValidationFailed(format!(
                    "Invalid refiner signature length: {} bytes (expected 64)",
                    sig.len()
                )));
            }
            // 2. Verify the ed25519 signature over the block hash
            //    The producer's public key is stored in their Account on-chain.
            //    This verification is done at the node level (apply_block) where
            //    AccountStore is available, not here in consensus-only validation.
            //    The signature length, producer non-zero, and structure checks
            //    are all we can verify without on-chain account data.
        } else if production_kind == opolys_core::BlockProductionKind::Genesis {
            unreachable!("height > 0 excludes genesis production kind");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use opolys_core::{ObjectId, Transaction, TransactionAction};
    use opolys_crypto::hash_to_object_id;

    #[test]
    fn transaction_root_empty() {
        let root = compute_transaction_root(&[]);
        assert_ne!(root, Hash::zero());
    }

    #[test]
    fn transaction_root_deterministic() {
        let tx = Transaction {
            tx_id: hash_to_object_id(b"test_tx"),
            sender: hash_to_object_id(b"sender"),
            action: TransactionAction::Transfer {
                recipient: hash_to_object_id(b"recipient"),
                amount: 100,
            },
            fee: 1000,
            signature: vec![1, 2, 3],
            signature_type: 0,
            nonce: 0,
            chain_id: opolys_core::MAINNET_CHAIN_ID,
            data: vec![],
            public_key: vec![],
        };
        let root1 = compute_transaction_root(std::slice::from_ref(&tx));
        let root2 = compute_transaction_root(&[tx]);
        assert_eq!(root1, root2);
    }

    #[test]
    fn body_roots_change_when_state_mutating_body_changes() {
        let evidence = DoubleSignEvidence {
            producer: hash_to_object_id(b"refiner"),
            producer_pubkey: vec![1; 32],
            height: 7,
            hash_a: Hash::from_bytes([2u8; 32]),
            signature_a: vec![3; 64],
            hash_b: Hash::from_bytes([4u8; 32]),
            signature_b: vec![5; 64],
        };
        let attestation = BlockAttestation {
            refiner: hash_to_object_id(b"attester"),
            refiner_pubkey: vec![6; 32],
            height: 6,
            block_hash: Hash::from_bytes([7u8; 32]),
            signature: vec![8; 64],
        };

        assert_ne!(
            compute_evidence_root(&[]).unwrap(),
            compute_evidence_root(&[evidence]).unwrap()
        );
        assert_ne!(
            compute_attestation_root(&[]).unwrap(),
            compute_attestation_root(&[attestation]).unwrap()
        );
    }

    fn test_header(height: u64, difficulty: u64) -> BlockHeader {
        BlockHeader {
            version: BLOCK_VERSION,
            height,
            previous_hash: Hash::zero(),
            state_root: Hash::from_bytes([0u8; 32]),
            transaction_root: Hash::from_bytes([0u8; 32]),
            evidence_root: compute_evidence_root(&[]).unwrap(),
            attestation_root: compute_attestation_root(&[]).unwrap(),
            genesis_ceremony_hash: compute_genesis_ceremony_hash(&None).unwrap(),
            timestamp: 1000,
            difficulty,
            suggested_fee: 1,
            extension_root: None,
            pow_proof: None,
            refiner_signature: None,
            producer: ObjectId(Hash::from_bytes([0u8; 32])),
        }
    }

    #[test]
    fn compute_block_hash_deterministic() {
        let header = BlockHeader {
            version: BLOCK_VERSION,
            height: 42,
            previous_hash: Hash::zero(),
            state_root: Hash::from_bytes([1u8; 32]),
            transaction_root: Hash::from_bytes([2u8; 32]),
            evidence_root: compute_evidence_root(&[]).unwrap(),
            attestation_root: compute_attestation_root(&[]).unwrap(),
            genesis_ceremony_hash: compute_genesis_ceremony_hash(&None).unwrap(),
            timestamp: 1700000000,
            difficulty: 100,
            suggested_fee: 1,
            extension_root: None,
            pow_proof: None,
            refiner_signature: None,
            producer: ObjectId(Hash::from_bytes([0u8; 32])),
        };
        let h1 = compute_block_hash(&header);
        let h2 = compute_block_hash(&header);
        assert_eq!(h1, h2);
        assert_ne!(h1, Hash::zero());
    }

    #[test]
    fn block_hash_differs_for_different_heights() {
        let base = test_header(1, 1);
        let h1 = compute_block_hash(&BlockHeader {
            height: 1,
            ..base.clone()
        });
        let h2 = compute_block_hash(&BlockHeader {
            height: 2,
            ..base.clone()
        });
        assert_ne!(h1, h2);
    }

    #[test]
    fn block_hash_chain_linkage() {
        let genesis_header = test_header(0, 1);
        let genesis_hash = compute_block_hash(&genesis_header);

        let block1_header = BlockHeader {
            height: 1,
            previous_hash: genesis_hash.clone(),
            state_root: Hash::from_bytes([1u8; 32]),
            transaction_root: Hash::from_bytes([1u8; 32]),
            timestamp: 1120,
            ..test_header(1, 1)
        };
        let block1_hash = compute_block_hash(&block1_header);

        let block2_header = BlockHeader {
            height: 2,
            previous_hash: block1_hash.clone(),
            state_root: Hash::from_bytes([2u8; 32]),
            transaction_root: Hash::from_bytes([2u8; 32]),
            timestamp: 1240,
            ..test_header(2, 1)
        };
        let _block2_hash = compute_block_hash(&block2_header);

        assert_eq!(block1_header.previous_hash, genesis_hash);
        assert_eq!(block2_header.previous_hash, block1_hash);
        assert_eq!(genesis_header.previous_hash, Hash::zero());
    }

    #[test]
    fn block_hash_includes_version() {
        let h1 = test_header(1, 1);
        let h2 = BlockHeader {
            version: BLOCK_VERSION + 1,
            ..h1.clone()
        };
        assert_ne!(compute_block_hash(&h1), compute_block_hash(&h2));
    }

    #[test]
    fn block_hash_includes_suggested_fee() {
        let h1 = test_header(1, 1);
        let h2 = BlockHeader {
            suggested_fee: 999,
            ..h1.clone()
        };
        assert_ne!(compute_block_hash(&h1), compute_block_hash(&h2));
    }

    #[test]
    fn block_hash_includes_body_roots() {
        let h1 = test_header(1, 1);
        let h2 = BlockHeader {
            evidence_root: Hash::from_bytes([9u8; 32]),
            ..h1.clone()
        };
        let h3 = BlockHeader {
            attestation_root: Hash::from_bytes([8u8; 32]),
            ..h1.clone()
        };
        let h4 = BlockHeader {
            genesis_ceremony_hash: Hash::from_bytes([7u8; 32]),
            ..h1.clone()
        };

        assert_ne!(compute_block_hash(&h1), compute_block_hash(&h2));
        assert_ne!(compute_block_hash(&h1), compute_block_hash(&h3));
        assert_ne!(compute_block_hash(&h1), compute_block_hash(&h4));
    }

    #[test]
    fn validate_block_rejects_pow_and_refiner_signature_together() {
        let header = BlockHeader {
            height: 1,
            timestamp: 1045,
            transaction_root: compute_transaction_root(&[]),
            producer: ObjectId(Hash::from_bytes([7u8; 32])),
            pow_proof: Some(vec![0; 8]),
            refiner_signature: Some(vec![0; 64]),
            ..test_header(1, 1)
        };
        let block = Block {
            header,
            transactions: vec![],
            slash_evidence: vec![],
            attestations: vec![],
            genesis_ceremony: None,
        };
        let err = validate_block(&block, 1, &Hash::zero(), 1000, 1, 1045).unwrap_err();
        assert!(err.to_string().contains("exactly one production proof"));
    }

    #[test]
    fn validate_block_rejects_zero_producer_after_genesis() {
        let header = BlockHeader {
            height: 1,
            timestamp: 1045,
            transaction_root: compute_transaction_root(&[]),
            refiner_signature: Some(vec![0; 64]),
            ..test_header(1, 1)
        };
        let block = Block {
            header,
            transactions: vec![],
            slash_evidence: vec![],
            attestations: vec![],
            genesis_ceremony: None,
        };
        let err = validate_block(&block, 1, &Hash::zero(), 1000, 1, 1045).unwrap_err();
        assert!(
            err.to_string()
                .contains("producer must be a non-zero ObjectId")
        );
    }

    #[test]
    fn validate_block_rejects_compressed_timestamp() {
        let header = BlockHeader {
            height: 1,
            timestamp: 1001,
            transaction_root: compute_transaction_root(&[]),
            producer: ObjectId(Hash::from_bytes([7u8; 32])),
            pow_proof: Some(vec![0; 8]),
            ..test_header(1, 1)
        };
        let block = Block {
            header,
            transactions: vec![],
            slash_evidence: vec![],
            attestations: vec![],
            genesis_ceremony: None,
        };
        let err = validate_block(&block, 1, &Hash::zero(), 1000, 1, 1001).unwrap_err();
        assert!(err.to_string().contains("too compressed"));
    }

    #[test]
    fn validate_block_accepts_target_derived_minimum_timestamp() {
        let header = BlockHeader {
            height: 1,
            timestamp: 1000 + minimum_block_timestamp_delta_secs(),
            transaction_root: compute_transaction_root(&[]),
            producer: ObjectId(Hash::from_bytes([7u8; 32])),
            refiner_signature: Some(vec![0; 64]),
            ..test_header(1, 1)
        };
        let block = Block {
            header,
            transactions: vec![],
            slash_evidence: vec![],
            attestations: vec![],
            genesis_ceremony: None,
        };
        assert!(
            validate_block(
                &block,
                1,
                &Hash::zero(),
                1000,
                1,
                1000 + minimum_block_timestamp_delta_secs()
            )
            .is_ok()
        );
    }

    #[test]
    fn validate_block_rejects_uncommitted_attestation_mutation() {
        let mut block = Block {
            header: BlockHeader {
                height: 0,
                ..test_header(0, 1)
            },
            transactions: vec![],
            slash_evidence: vec![],
            attestations: vec![],
            genesis_ceremony: None,
        };
        set_body_roots(&mut block).unwrap();
        block.attestations.push(BlockAttestation {
            refiner: hash_to_object_id(b"attester"),
            refiner_pubkey: vec![1; 32],
            height: 0,
            block_hash: Hash::from_bytes([2u8; 32]),
            signature: vec![3; 64],
        });

        let err = validate_block(&block, 0, &Hash::zero(), 0, 1, 1001).unwrap_err();
        assert!(err.to_string().contains("Attestation root mismatch"));
    }

    #[test]
    fn validate_block_rejects_uncommitted_slash_evidence_mutation() {
        let mut block = Block {
            header: BlockHeader {
                height: 0,
                ..test_header(0, 1)
            },
            transactions: vec![],
            slash_evidence: vec![],
            attestations: vec![],
            genesis_ceremony: None,
        };
        set_body_roots(&mut block).unwrap();
        block.slash_evidence.push(DoubleSignEvidence {
            producer: hash_to_object_id(b"refiner"),
            producer_pubkey: vec![1; 32],
            height: 0,
            hash_a: Hash::from_bytes([2u8; 32]),
            signature_a: vec![3; 64],
            hash_b: Hash::from_bytes([4u8; 32]),
            signature_b: vec![5; 64],
        });

        let err = validate_block(&block, 0, &Hash::zero(), 0, 1, 1001).unwrap_err();
        assert!(err.to_string().contains("Evidence root mismatch"));
    }

    #[test]
    fn suggested_fee_ema() {
        use crate::emission::compute_suggested_fee;
        // Initial fee
        let fee = compute_suggested_fee(0, 0, 0);
        assert_eq!(fee, 1); // MIN_FEE floor

        // EMA: (10000 + 9 * 1000) / 10 = 1900
        let fee = compute_suggested_fee(10_000, 1, 1_000);
        assert_eq!(fee, 1900);
    }

    #[test]
    fn format_opl_amounts() {
        assert_eq!(format_opl(1_000_000), "1.000000 OPL");
        assert_eq!(format_opl(0), "0.000000 OPL");
        assert_eq!(format_opl(1), "0.000001 OPL");
        assert_eq!(format_opl(312 * 1_000_000), "312.000000 OPL");
    }
}
