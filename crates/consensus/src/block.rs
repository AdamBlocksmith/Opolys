use opolys_core::{Hash, Block, BlockHeader, ConsensusPhase, FLAKES_PER_OPL, POS_FINALITY_BLOCKS};
use borsh::{BorshSerialize, BorshDeserialize};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct BlockInfo {
    pub header: BlockHeader,
    pub transaction_count: u32,
    pub total_fees_burned: u64,
}

impl BlockInfo {
    pub fn from_block(block: &Block) -> Self {
        let total_fees_burned: u64 = block.transactions.iter().map(|tx| tx.fee).sum();
        Self {
            header: block.header.clone(),
            transaction_count: block.transactions.len() as u32,
            total_fees_burned,
        }
    }
}

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize, Serialize, Deserialize, PartialEq, Eq)]
pub enum BlockStatus {
    Pending,
    Confirmed,
    Finalized,
    Orphaned,
}

pub fn compute_transaction_root(transactions: &[opolys_core::Transaction]) -> Hash {
    let mut hasher = opolys_crypto::Blake3Hasher::new();
    for tx in transactions {
        hasher.update(&tx.tx_id.0 .0);
        hasher.update(&tx.fee.to_be_bytes());
        hasher.update(&tx.nonce.to_be_bytes());
    }
    hasher.finalize()
}

pub fn format_opl(flakes: u64) -> String {
    let whole = flakes / FLAKES_PER_OPL;
    let frac = flakes % FLAKES_PER_OPL;
    format!("{}.{:06} OPL", whole, frac)
}

#[cfg(test)]
mod tests {
    use super::*;
    use opolys_core::{Transaction, TransactionAction, ObjectId};
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
    fn format_opl_amounts() {
        assert_eq!(format_opl(1_000_000), "1.000000 OPL");
        assert_eq!(format_opl(0), "0.000000 OPL");
        assert_eq!(format_opl(1), "0.000001 OPL");
        assert_eq!(format_opl(440 * 1_000_000), "440.000000 OPL");
    }
}