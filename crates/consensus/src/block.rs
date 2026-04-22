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

use opolys_core::{Hash, Block, BlockHeader, FLAKES_PER_OPL};
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
/// not the other way around.
pub fn compute_block_hash(header: &BlockHeader) -> Hash {
    let mut hasher = Blake3Hasher::new();
    // Hash every field of the header in a fixed order for determinism.
    // pow_proof and validator_signature are excluded because:
    // - pow_proof is set AFTER mining (the proof must satisfy the hash, not vice versa)
    // - validator_signature (ed25519) is appended after block producer selection
    hasher.update(&header.height.to_be_bytes());
    hasher.update(&header.previous_hash.0);
    hasher.update(&header.state_root.0);
    hasher.update(&header.transaction_root.0);
    hasher.update(&header.timestamp.to_be_bytes());
    hasher.update(&header.difficulty.to_be_bytes());
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
            nonce: 0,
            data: vec![],
        };
        let root1 = compute_transaction_root(&[tx.clone()]);
        let root2 = compute_transaction_root(&[tx]);
        assert_eq!(root1, root2);
    }

    #[test]
    fn compute_block_hash_deterministic() {
        let header = BlockHeader {
            height: 42,
            previous_hash: Hash::zero(),
            state_root: Hash::from_bytes([1u8; 32]),
            transaction_root: Hash::from_bytes([2u8; 32]),
            timestamp: 1700000000,
            difficulty: 100,
            pow_proof: None,
            validator_signature: None,
        };
        let h1 = compute_block_hash(&header);
        let h2 = compute_block_hash(&header);
        assert_eq!(h1, h2);
        assert_ne!(h1, Hash::zero());
    }

    #[test]
    fn block_hash_differs_for_different_heights() {
        let base = BlockHeader {
            height: 1,
            previous_hash: Hash::zero(),
            state_root: Hash::zero(),
            transaction_root: Hash::zero(),
            timestamp: 100,
            difficulty: 1,
            pow_proof: None,
            validator_signature: None,
        };
        let h1 = compute_block_hash(&BlockHeader { height: 1, ..base.clone() });
        let h2 = compute_block_hash(&BlockHeader { height: 2, ..base.clone() });
        assert_ne!(h1, h2);
    }

    #[test]
    fn block_hash_chain_linkage() {
        // Simulate a 3-block chain where each block references the previous hash
        let genesis_header = BlockHeader {
            height: 0,
            previous_hash: Hash::zero(),
            state_root: Hash::from_bytes([0u8; 32]),
            transaction_root: Hash::from_bytes([0u8; 32]),
            timestamp: 1000,
            difficulty: 1,
            pow_proof: None,
            validator_signature: None,
        };
        let genesis_hash = compute_block_hash(&genesis_header);

        let block1_header = BlockHeader {
            height: 1,
            previous_hash: genesis_hash.clone(),
            state_root: Hash::from_bytes([1u8; 32]),
            transaction_root: Hash::from_bytes([1u8; 32]),
            timestamp: 1120,
            difficulty: 1,
            pow_proof: None,
            validator_signature: None,
        };
        let block1_hash = compute_block_hash(&block1_header);

        let block2_header = BlockHeader {
            height: 2,
            previous_hash: block1_hash.clone(),
            state_root: Hash::from_bytes([2u8; 32]),
            transaction_root: Hash::from_bytes([2u8; 32]),
            timestamp: 1240,
            difficulty: 1,
            pow_proof: None,
            validator_signature: None,
        };
        let _block2_hash = compute_block_hash(&block2_header);

        // Block 1 must reference genesis hash
        assert_eq!(block1_header.previous_hash, genesis_hash);
        // Block 2 must reference block 1 hash
        assert_eq!(block2_header.previous_hash, block1_hash);
        // Genesis has zero previous hash
        assert_eq!(genesis_header.previous_hash, Hash::zero());
    }

    #[test]
    fn format_opl_amounts() {
        assert_eq!(format_opl(1_000_000), "1.000000 OPL");
        assert_eq!(format_opl(0), "0.000000 OPL");
        assert_eq!(format_opl(1), "0.000001 OPL");
        assert_eq!(format_opl(440 * 1_000_000), "440.000000 OPL");
    }
}