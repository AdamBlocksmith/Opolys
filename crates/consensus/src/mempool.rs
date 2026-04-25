//! # Fee-prioritized transaction mempool.
//!
//! Opolys uses a market-driven fee model: transactions specify their own fee,
//! and block producers select the highest-fees-first. Fees are **burned** (not
//! collected by miners or validators), creating deflationary pressure that
//! scales with network usage. No governance sets fee minimums — the market
//! decides.
//!
//! The mempool enforces per-account transaction limits and a global size cap.
//! When the pool is full, the lowest-priority transactions are evicted to make
//! room for higher-fee arrivals.

use opolys_core::{
    Transaction, ObjectId, OpolysError,
    TX_MAX_SIZE_BYTES, MEMPOOL_MAX_SIZE_BYTES, MEMPOOL_MAX_TXS_PER_ACCOUNT,
};
use std::collections::HashMap;

/// A single transaction entry in the mempool, annotated with its priority
/// score (derived from fee density) and the time it was submitted.
#[derive(Debug, Clone)]
pub struct MempoolEntry {
    /// The full transaction data.
    pub transaction: Transaction,
    /// Priority score used for ordering — higher scores are included in
    /// blocks first. Typically derived from fee-to-size ratio.
    pub priority_score: f64,
    /// Unix timestamp when the transaction entered the mempool, used as a
    /// tiebreaker (earlier transactions win among equal-priority entries).
    pub submitted_at: u64,
}

/// The global transaction mempool, storing pending transactions awaiting
/// inclusion in a block.
///
/// Transactions are ordered by `priority_score` (descending), then by
/// `submitted_at` (ascending) as a tiebreaker. The pool enforces:
/// - A maximum serialized size (`MEMPOOL_MAX_SIZE_BYTES`).
/// - A per-account transaction count limit (`MEMPOOL_MAX_TXS_PER_ACCOUNT`).
/// - A per-transaction size limit (`TX_MAX_SIZE_BYTES`).
///
/// When the pool is full, low-priority entries are evicted to make room.
pub struct Mempool {
    /// Transaction entries keyed by their transaction ID.
    entries: HashMap<ObjectId, MempoolEntry>,
    /// Number of transactions currently in the pool per sender account.
    account_tx_counts: HashMap<ObjectId, usize>,
    /// Total serialized byte size of all entries in the pool.
    total_size: usize,
}

impl Mempool {
    /// Create an empty mempool.
    pub fn new() -> Self {
        Mempool {
            entries: HashMap::new(),
            account_tx_counts: HashMap::new(),
            total_size: 0,
        }
    }

    /// Attempt to add a transaction to the mempool.
    ///
    /// Fails if:
    /// - The transaction exceeds `TX_MAX_SIZE_BYTES`.
    /// - The sender already has `MEMPOOL_MAX_TXS_PER_ACCOUNT` pending transactions.
    /// - A transaction with the same ID already exists (duplicate).
    /// - The pool cannot free enough space by eviction.
    pub fn add_transaction(
        &mut self,
        tx: Transaction,
        priority_score: f64,
        submitted_at: u64,
    ) -> Result<(), OpolysError> {
        let tx_size = borsh::to_vec(&tx).map(|v| v.len()).unwrap_or(0);
        if tx_size > TX_MAX_SIZE_BYTES {
            return Err(OpolysError::InvalidTransaction(format!(
                "Transaction too large: {} bytes", tx_size
            )));
        }

        // Enforce per-account transaction limit to prevent spam.
        let sender_count = self.account_tx_counts.get(&tx.sender).copied().unwrap_or(0);
        if sender_count >= MEMPOOL_MAX_TXS_PER_ACCOUNT {
            return Err(OpolysError::InvalidTransaction(format!(
                "Too many transactions from account: {}", tx.sender.to_hex()
            )));
        }

        // Reject duplicate transactions by ID.
        if self.entries.contains_key(&tx.tx_id) {
            return Err(OpolysError::InvalidTransaction("Duplicate transaction".to_string()));
        }

        // If the pool is over capacity, try evicting lowest-priority entries.
        if self.total_size + tx_size > MEMPOOL_MAX_SIZE_BYTES {
            self.evict_lowest_priority(tx_size);
        }

        // Still over capacity after eviction — reject the transaction.
        if self.total_size + tx_size > MEMPOOL_MAX_SIZE_BYTES {
            return Err(OpolysError::MempoolFull);
        }

        self.account_tx_counts.entry(tx.sender.clone())
            .and_modify(|c| *c += 1)
            .or_insert(1);

        self.total_size += tx_size;
        self.entries.insert(tx.tx_id.clone(), MempoolEntry {
            transaction: tx,
            priority_score,
            submitted_at,
        });

        Ok(())
    }

    /// Remove a transaction by ID. Returns the transaction data if found,
    /// and decrements the sender's per-account counter.
    pub fn remove_transaction(&mut self, tx_id: &ObjectId) -> Option<Transaction> {
        if let Some(entry) = self.entries.remove(tx_id) {
            let sender = &entry.transaction.sender;
            if let Some(count) = self.account_tx_counts.get_mut(sender) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    self.account_tx_counts.remove(sender);
                }
            }
            let tx_size = borsh::to_vec(&entry.transaction).map(|v| v.len()).unwrap_or(0);
            self.total_size = self.total_size.saturating_sub(tx_size);
            Some(entry.transaction)
        } else {
            None
        }
    }

    /// Look up a transaction by ID.
    pub fn get_transaction(&self, tx_id: &ObjectId) -> Option<&Transaction> {
        self.entries.get(tx_id).map(|e| &e.transaction)
    }

    /// Return all transactions sorted by descending priority score, then
    /// ascending submission time. Block producers iterate this list to
    /// fill blocks with the most valuable transactions first.
    pub fn get_ordered_transactions(&self) -> Vec<&Transaction> {
        let mut entries: Vec<&MempoolEntry> = self.entries.values().collect();
        // Sort by priority (descending), then by submission time (ascending).
        entries.sort_by(|a, b| {
            b.priority_score.partial_cmp(&a.priority_score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.submitted_at.cmp(&b.submitted_at))
        });
        entries.iter().map(|e| &e.transaction).collect()
    }

    /// Number of transactions currently in the mempool.
    pub fn transaction_count(&self) -> usize {
        self.entries.len()
    }

    /// Total serialized byte size of all transactions in the pool.
    pub fn total_size(&self) -> usize {
        self.total_size
    }

    /// Evict the lowest-priority transactions until at least `needed_space`
    /// bytes have been freed. Eviction order is ascending priority score,
    /// then ascending submission time (oldest first among equal priority).
    fn evict_lowest_priority(&mut self, needed_space: usize) {
        let mut entries: Vec<(ObjectId, f64, u64, usize)> = self.entries.iter()
            .map(|(id, e)| {
                let size = borsh::to_vec(&e.transaction).map(|v| v.len()).unwrap_or(0);
                (id.clone(), e.priority_score, e.submitted_at, size)
            })
            .collect();

        // Sort ascending by priority (lowest first), then by submission time
        // (oldest first) to evict the least valuable transactions.
        entries.sort_by(|a, b| {
            a.1.partial_cmp(&b.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.2.cmp(&b.2))
        });

        let mut freed = 0usize;
        for (id, _, _, size) in entries {
            if freed >= needed_space {
                break;
            }
            self.remove_transaction(&id);
            freed += size;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opolys_core::{FlakeAmount, TransactionAction};
    use opolys_crypto::hash_to_object_id;

    fn make_tx(sender_seed: &[u8], nonce: u64, fee: FlakeAmount) -> Transaction {
        Transaction {
            tx_id: hash_to_object_id(format!("{:?}_{}", sender_seed, nonce).as_bytes()),
            sender: hash_to_object_id(sender_seed),
            action: TransactionAction::Transfer {
                recipient: hash_to_object_id(b"recipient"),
                amount: 100,
            },
            fee,
            signature: vec![],
            nonce,
            data: vec![],
            signature_type: opolys_core::SIGNATURE_TYPE_ED25519,
        }
    }

    #[test]
    fn add_and_remove_transaction() {
        let mut mempool = Mempool::new();
        let tx = make_tx(b"alice", 0, 100);
        let tx_id = tx.tx_id.clone();

        mempool.add_transaction(tx, 1.0, 0).unwrap();
        assert_eq!(mempool.transaction_count(), 1);

        let removed = mempool.remove_transaction(&tx_id);
        assert!(removed.is_some());
        assert_eq!(mempool.transaction_count(), 0);
    }

    #[test]
    fn priority_ordering() {
        let mut mempool = Mempool::new();
        let tx1 = make_tx(b"alice", 0, 50);
        let tx2 = make_tx(b"bob", 0, 100);
        let tx3 = make_tx(b"charlie", 0, 75);

        mempool.add_transaction(tx1, 1.0, 0).unwrap();
        mempool.add_transaction(tx2, 3.0, 0).unwrap();
        mempool.add_transaction(tx3, 2.0, 0).unwrap();

        let ordered = mempool.get_ordered_transactions();
        assert_eq!(ordered[0].fee, 100);
        assert_eq!(ordered[1].fee, 75);
        assert_eq!(ordered[2].fee, 50);
    }

    #[test]
    fn duplicate_transaction_rejected() {
        let mut mempool = Mempool::new();
        let tx = make_tx(b"alice", 0, 100);
        let tx2 = tx.clone();

        mempool.add_transaction(tx, 1.0, 0).unwrap();
        assert!(mempool.add_transaction(tx2, 1.0, 0).is_err());
    }

    #[test]
    fn per_account_limit() {
        let mut mempool = Mempool::new();
        for i in 0..(MEMPOOL_MAX_TXS_PER_ACCOUNT + 1) {
            let tx = make_tx(b"alice", i as u64, 100);
            if i < MEMPOOL_MAX_TXS_PER_ACCOUNT {
                assert!(mempool.add_transaction(tx, 1.0, 0).is_ok());
            } else {
                assert!(mempool.add_transaction(tx, 1.0, 0).is_err());
            }
        }
    }
}