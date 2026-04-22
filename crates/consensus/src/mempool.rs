use opolys_core::{
    Transaction, TransactionAction, ObjectId, FleckAmount, OpolysError,
    TX_MAX_SIZE_BYTES, MEMPOOL_MAX_SIZE_BYTES, MEMPOOL_MAX_TXS_PER_ACCOUNT,
};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct MempoolEntry {
    pub transaction: Transaction,
    pub priority_score: f64,
    pub submitted_at: u64,
}

pub struct Mempool {
    entries: HashMap<ObjectId, MempoolEntry>,
    account_tx_counts: HashMap<ObjectId, usize>,
    total_size: usize,
}

impl Mempool {
    pub fn new() -> Self {
        Mempool {
            entries: HashMap::new(),
            account_tx_counts: HashMap::new(),
            total_size: 0,
        }
    }

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

        let sender_count = self.account_tx_counts.get(&tx.sender).copied().unwrap_or(0);
        if sender_count >= MEMPOOL_MAX_TXS_PER_ACCOUNT {
            return Err(OpolysError::InvalidTransaction(format!(
                "Too many transactions from account: {}", tx.sender.to_hex()
            )));
        }

        if self.entries.contains_key(&tx.tx_id) {
            return Err(OpolysError::InvalidTransaction("Duplicate transaction".to_string()));
        }

        if self.total_size + tx_size > MEMPOOL_MAX_SIZE_BYTES {
            self.evict_lowest_priority(tx_size);
        }

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

    pub fn get_transaction(&self, tx_id: &ObjectId) -> Option<&Transaction> {
        self.entries.get(tx_id).map(|e| &e.transaction)
    }

    pub fn get_ordered_transactions(&self) -> Vec<&Transaction> {
        let mut entries: Vec<&MempoolEntry> = self.entries.values().collect();
        entries.sort_by(|a, b| {
            b.priority_score.partial_cmp(&a.priority_score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.submitted_at.cmp(&b.submitted_at))
        });
        entries.iter().map(|e| &e.transaction).collect()
    }

    pub fn transaction_count(&self) -> usize {
        self.entries.len()
    }

    pub fn total_size(&self) -> usize {
        self.total_size
    }

    fn evict_lowest_priority(&mut self, needed_space: usize) {
        let mut entries: Vec<(ObjectId, f64, u64, usize)> = self.entries.iter()
            .map(|(id, e)| {
                let size = borsh::to_vec(&e.transaction).map(|v| v.len()).unwrap_or(0);
                (id.clone(), e.priority_score, e.submitted_at, size)
            })
            .collect();

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
    use opolys_crypto::hash_to_object_id;

    fn make_tx(sender_seed: &[u8], nonce: u64, fee: FleckAmount) -> Transaction {
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