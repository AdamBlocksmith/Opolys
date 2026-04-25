//! # Block hashing, transaction roots, and display formatting.
//!
//! Opolys blocks are linked by Blake3-256 hashes of their headers. Every header
//! field — except `pow_proof` and `validator_signature`, which are set after
//! the block is produced — is hashed in a fixed order for deterministic
//! identification. Transaction integrity is guaranteed by a streaming Merkle-
//! like root over each transaction's ID, fee, and nonce.
//!
//! Fees included in blocks are **burned** rather than collected by any party,
//! keeping the fee market pure and deflationary.

use opolys_core::{Hash, ObjectId, Block, BlockHeader, FLAKES_PER_OPL, BLOCK_VERSION, MAX_TRANSACTIONS_PER_BLOCK, MAX_BLOCK_SIZE_BYTES, MAX_TX_DATA_SIZE_BYTES, MAX_FUTURE_BLOCK_TIME_SECS, OpolysError};
use borsh::{BorshSerialize, BorshDeserialize};
use serde::{Deserialize, Serialize};
use opolys_crypto::Blake3Hasher;

/// Metadata extracted from a block for indexing and querying.
///
/// Captures the header, transaction count, and total fees **burned** in the
/// block. Because Opolys burns all fees, `total_fees_burned` directly
/// measures the deflationary pressure applied by this block.
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct BlockInfo {
    /// The block header containing height, hashes, timestamp, and difficulty.
    pub header: BlockHeader,
    /// Number of transactions included in this block.
    pub transaction_count: u32,
    /// Sum of all transaction fees in this block — burned, not collected.
    pub total_fees_burned: u64,
}

impl BlockInfo {
    /// Derive `BlockInfo` from a complete block by summing all transaction fees.
    pub fn from_block(block: &Block) -> Self {
        let total_fees_burned: u64 = block.transactions.iter().map(|tx| tx.fee).sum();
        Self {
            header: block.header.clone(),
            transaction_count: block.transactions.len() as u32,
            total_fees_burned,
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
/// Note: `pow_proof` and `validator_signature` are intentionally excluded
/// because they are populated after mining — the proof must satisfy the hash,
/// not the other way around. The `version`, `suggested_fee`, and
/// `extension_root` fields are included to bind the PoW to the complete
/// header state.
pub fn compute_block_hash(header: &BlockHeader) -> Hash {
    let mut hasher = Blake3Hasher::new();
    // Hash every field of the header in a fixed order for determinism.
    // pow_proof and validator_signature are excluded because:
    // - pow_proof is set AFTER mining (the proof must satisfy the hash, not vice versa)
    // - validator_signature (ed25519) is appended after block producer selection
    // producer IS included because it identifies who earns the block reward.
    hasher.update(&header.version.to_be_bytes());
    hasher.update(&header.height.to_be_bytes());
    hasher.update(&header.previous_hash.0);
    hasher.update(header.producer.as_bytes());
    hasher.update(&header.state_root.0);
    hasher.update(&header.transaction_root.0);
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
    for tx in transactions {
        // Each transaction is committed by its unique ID, fee, and nonce.
        hasher.update(&tx.tx_id.0 .0);
        hasher.update(&tx.fee.to_be_bytes());
        hasher.update(&tx.nonce.to_be_bytes());
    }
    hasher.finalize()
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
/// 1. **Version**: Must match `BLOCK_VERSION` (currently 1).
/// 2. **Height**: Must be exactly `expected_height` (parent height + 1).
/// 3. **Previous hash**: Must match the parent block's hash (or `Hash::zero()` for genesis).
/// 4. **Timestamp**: Must be greater than `parent_timestamp` and within
///    `MAX_FUTURE_BLOCK_TIME_SECS` of the current wall clock.
/// 5. **Difficulty**: Must match `expected_difficulty`.
/// 6. **Transaction count**: Must not exceed `MAX_TRANSACTIONS_PER_BLOCK`.
/// 7. **Block size**: Must not exceed `MAX_BLOCK_SIZE_BYTES`.
/// 8. **Transaction root**: Must match `compute_transaction_root(block.transactions)`.
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

    // 4. Timestamp check: must be strictly greater than parent, and not too far in the future
    if block.header.timestamp <= parent_timestamp && expected_height > 0 {
        return Err(OpolysError::BlockValidationFailed(format!(
            "Block timestamp {} must be greater than parent timestamp {}",
            block.header.timestamp, parent_timestamp
        )));
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
            block.transactions.len(), MAX_TRANSACTIONS_PER_BLOCK
        )));
    }

    // 7. Block size check
    let block_size = borsh::to_vec(block)
        .map(|v| v.len())
        .unwrap_or(usize::MAX);
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

    // 9. Duplicate transaction check
    let mut seen_tx_ids = std::collections::HashSet::new();
    for tx in &block.transactions {
        if !seen_tx_ids.insert(&tx.tx_id) {
            return Err(OpolysError::BlockValidationFailed(format!(
                "Duplicate transaction {} in block {}",
                tx.tx_id.to_hex(), block.header.height
            )));
        }
    }

    // 10. Transaction data size check
    for tx in &block.transactions {
        if tx.data.len() > MAX_TX_DATA_SIZE_BYTES {
            return Err(OpolysError::BlockValidationFailed(format!(
                "Transaction {} data too large: {} bytes > {} bytes",
                tx.tx_id.to_hex(), tx.data.len(), MAX_TX_DATA_SIZE_BYTES
            )));
        }
    }

    // 11. Fee minimum check
    for tx in &block.transactions {
        if tx.fee < opolys_core::MIN_FEE {
            return Err(OpolysError::BlockValidationFailed(format!(
                "Transaction {} fee below minimum: {} < {}",
                tx.tx_id.to_hex(), tx.fee, opolys_core::MIN_FEE
            )));
        }
    }

// 12. Block proof check:
    // - PoW blocks: verify the EVO-OMAP proof-of-work
    // - PoS blocks: verify the validator's ed25519 signature over the block hash
    //   The producer's public key must be stored on-chain in the AccountStore.
    // - Genesis block (height 0): skip both
    if expected_height > 0 {
        if block.header.pow_proof.is_some() {
            // PoW block — verify the proof-of-work
            if let Err(e) = crate::pow::verify_pow(&block.header, block.header.difficulty) {
                return Err(e);
            }
        } else if block.header.validator_signature.is_some() {
            // PoS block — verify the validator's ed25519 signature
            // 1. The signature must be exactly 64 bytes (ed25519)
            let sig = block.header.validator_signature.as_ref().unwrap();
            if sig.len() != 64 {
                return Err(OpolysError::BlockValidationFailed(format!(
                    "Invalid validator signature length: {} bytes (expected 64)",
                    sig.len()
                )));
            }
            // 2. The producer must not be the zero ObjectId
            if block.header.producer.0.is_zero() {
                return Err(OpolysError::BlockValidationFailed(
                    "PoS block producer must be a valid validator ObjectId".to_string()
                ));
            }
            // 3. Verify the ed25519 signature over the block hash
            //    The producer's public key is stored in their Account on-chain.
            //    This verification is done at the node level (apply_block) where
            //    AccountStore is available, not here in consensus-only validation.
            //    The signature length, producer non-zero, and structure checks
            //    are all we can verify without on-chain account data.
        } else {
            // Neither PoW proof nor validator signature — invalid
            return Err(OpolysError::BlockValidationFailed(
                "Block must have either pow_proof or validator_signature".to_string()
            ));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use opolys_core::{Transaction, TransactionAction};
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
            data: vec![],
            public_key: vec![],
        };
        let root1 = compute_transaction_root(&[tx.clone()]);
        let root2 = compute_transaction_root(&[tx]);
        assert_eq!(root1, root2);
    }

    fn test_header(height: u64, difficulty: u64) -> BlockHeader {
        BlockHeader {
            version: 1,
            height,
            previous_hash: Hash::zero(),
            state_root: Hash::from_bytes([0u8; 32]),
            transaction_root: Hash::from_bytes([0u8; 32]),
            timestamp: 1000,
            difficulty,
            suggested_fee: 1,
            extension_root: None,
            pow_proof: None,
            validator_signature: None,
            producer: ObjectId(Hash::from_bytes([0u8; 32])),
        }
    }

    #[test]
    fn compute_block_hash_deterministic() {
        let header = BlockHeader {
            version: 1,
            height: 42,
            previous_hash: Hash::zero(),
            state_root: Hash::from_bytes([1u8; 32]),
            transaction_root: Hash::from_bytes([2u8; 32]),
            timestamp: 1700000000,
            difficulty: 100,
            suggested_fee: 1,
            extension_root: None,
            pow_proof: None,
            validator_signature: None,
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
        let h1 = compute_block_hash(&BlockHeader { height: 1, ..base.clone() });
        let h2 = compute_block_hash(&BlockHeader { height: 2, ..base.clone() });
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
        let h2 = BlockHeader { version: 2, ..h1.clone() };
        assert_ne!(compute_block_hash(&h1), compute_block_hash(&h2));
    }

    #[test]
    fn block_hash_includes_suggested_fee() {
        let h1 = test_header(1, 1);
        let h2 = BlockHeader { suggested_fee: 999, ..h1.clone() };
        assert_ne!(compute_block_hash(&h1), compute_block_hash(&h2));
    }

    #[test]
    fn suggested_fee_ema() {
        use crate::emission::compute_suggested_fee;
        // Initial fee
        let fee = compute_suggested_fee(0, 0);
        assert_eq!(fee, 1); // MIN_FEE floor

        // EMA: (10000 + 9 * 1000) / 10 = 1900
        let fee = compute_suggested_fee(10_000, 1_000);
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