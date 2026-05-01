//! Transaction construction and signing for the Opolys blockchain.
//!
//! Provides `TransactionSigner`, a stateless utility that creates fully signed
//! `Transaction` objects ready for inclusion in a block. Each method serializes
//! the transaction data with Borsh, signs it with the sender's ed25519 key,
//! and computes a deterministic transaction ID via Blake3-256.
//!
//! Transactions are:
//! - **Transfer** — move OPL between accounts (fees are burned, not collected)
//! - **RefinerBond** — lock OPL as stake to become a refiner (min 1 OPL per entry)
//! - **RefinerUnbond** — release stake using FIFO order (oldest entries first)

use opolys_core::{FlakeAmount, ObjectId, Transaction, TransactionAction, SIGNATURE_TYPE_ED25519};
use crate::key::KeyPair;
use opolys_crypto::hash_to_object_id;

/// Stateless transaction signer — all methods are associated functions.
///
/// Creates fully signed `Transaction` objects. The sender's nonce must match
/// the account's current nonce on-chain or the transaction will be rejected
/// during execution.
pub struct TransactionSigner;

impl TransactionSigner {
    /// Create a signed transfer transaction.
    ///
    /// Moves `amount` OPL (in flakes) from sender to recipient. The `fee` is
    /// **burned** (permanently removed from supply), not collected by any refiner.
    /// This is a core Opolys design choice: fees are market-driven and burned.
    ///
    /// `chain_id` must match the target network (`MAINNET_CHAIN_ID` for mainnet).
    /// It is included in both the tx_id hash and the signed data to prevent
    /// cross-chain replay attacks.
    pub fn create_transfer(
        sender: &KeyPair,
        recipient: ObjectId,
        amount: FlakeAmount,
        fee: FlakeAmount,
        nonce: u64,
        chain_id: u64,
    ) -> Transaction {
        let action = TransactionAction::Transfer { recipient, amount };
        let sender_id = sender.object_id().clone();

        let tx_id = Self::compute_tx_id(&sender_id, &action, fee, nonce, chain_id);
        let unsigned_data = borsh::to_vec(&(sender_id.clone(), &action, fee, nonce, chain_id)).unwrap_or_default();
        let signature = sender.sign(&unsigned_data);

        Transaction {
            tx_id,
            sender: sender_id,
            action,
            fee,
            signature,
            signature_type: SIGNATURE_TYPE_ED25519,
            nonce,
            chain_id,
            data: vec![],
            public_key: sender.public_key_bytes(),
        }
    }

    /// Create a signed refiner bond transaction.
    ///
    /// Locks `amount` OPL (in flakes) as refiner stake. If the sender is
    /// already a refiner, this creates a new bond entry (top-up) with its
    /// own seniority clock starting from zero. Each new entry must be at least
    /// `MIN_BOND_STAKE` (1 OPL).
    ///
    /// `chain_id` must match the target network to prevent cross-chain replay attacks.
    pub fn create_refiner_bond(
        sender: &KeyPair,
        amount: FlakeAmount,
        fee: FlakeAmount,
        nonce: u64,
        chain_id: u64,
    ) -> Transaction {
        let action = TransactionAction::RefinerBond { amount };
        let sender_id = sender.object_id().clone();

        let tx_id = Self::compute_tx_id(&sender_id, &action, fee, nonce, chain_id);
        let unsigned_data = borsh::to_vec(&(sender_id.clone(), &action, fee, nonce, chain_id)).unwrap_or_default();
        let signature = sender.sign(&unsigned_data);

        Transaction {
            tx_id,
            sender: sender_id,
            action,
            fee,
            signature,
            signature_type: SIGNATURE_TYPE_ED25519,
            nonce,
            chain_id,
            data: vec![],
            public_key: sender.public_key_bytes(),
        }
    }

    /// Create a signed refiner unbond transaction for FIFO amount-based unbonding.
    ///
    /// Unbonds `amount` Flakes from the refiner's stake using FIFO order —
    /// the oldest entries are consumed first. If the amount exceeds an entry's
    /// stake, that entry is fully consumed and the remainder comes from the next
    /// oldest. After a UNBONDING_DELAY_BLOCKS delay, the unbonded stake is
    /// returned to the sender's wallet.
    ///
    /// `chain_id` must match the target network to prevent cross-chain replay attacks.
    pub fn create_refiner_unbond(
        sender: &KeyPair,
        amount: FlakeAmount,
        fee: FlakeAmount,
        nonce: u64,
        chain_id: u64,
    ) -> Transaction {
        let action = TransactionAction::RefinerUnbond { amount };
        let sender_id = sender.object_id().clone();

        let tx_id = Self::compute_tx_id(&sender_id, &action, fee, nonce, chain_id);
        let unsigned_data = borsh::to_vec(&(sender_id.clone(), &action, fee, nonce, chain_id)).unwrap_or_default();
        let signature = sender.sign(&unsigned_data);

        Transaction {
            tx_id,
            sender: sender_id,
            action,
            fee,
            signature,
            signature_type: SIGNATURE_TYPE_ED25519,
            nonce,
            chain_id,
            data: vec![],
            public_key: sender.public_key_bytes(),
        }
    }

    /// Deterministic transaction ID derived from sender, action, fee, nonce, and chain_id
    /// via Blake3-256. Including chain_id prevents cross-chain replay attacks.
    fn compute_tx_id(
        sender: &ObjectId,
        action: &TransactionAction,
        fee: FlakeAmount,
        nonce: u64,
        chain_id: u64,
    ) -> ObjectId {
        let mut data = sender.0.to_hex().as_bytes().to_vec();
        data.extend_from_slice(borsh::to_vec(action).unwrap_or_default().as_slice());
        data.extend_from_slice(&fee.to_be_bytes());
        data.extend_from_slice(&nonce.to_be_bytes());
        data.extend_from_slice(&chain_id.to_be_bytes());
        hash_to_object_id(&data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opolys_core::{FLAKES_PER_OPL, MAINNET_CHAIN_ID};

    #[test]
    fn create_transfer_transaction() {
        let keypair = KeyPair::generate();
        let recipient = hash_to_object_id(b"recipient");
        let tx = TransactionSigner::create_transfer(
            &keypair,
            recipient,
            FLAKES_PER_OPL,
            FLAKES_PER_OPL / 10,
            0,
            MAINNET_CHAIN_ID,
        );
        assert_eq!(tx.nonce, 0);
        assert_eq!(tx.fee, FLAKES_PER_OPL / 10);
        assert_eq!(tx.chain_id, MAINNET_CHAIN_ID);
        assert_eq!(tx.signature_type, SIGNATURE_TYPE_ED25519);
        assert!(matches!(tx.action, TransactionAction::Transfer { .. }));
    }

    #[test]
    fn create_bond_transaction() {
        let keypair = KeyPair::generate();
        let bond_amount = FLAKES_PER_OPL;
        let tx = TransactionSigner::create_refiner_bond(
            &keypair,
            bond_amount,
            FLAKES_PER_OPL,
            0,
            MAINNET_CHAIN_ID,
        );
        assert!(matches!(tx.action, TransactionAction::RefinerBond { amount } if amount == bond_amount));
        assert_eq!(tx.chain_id, MAINNET_CHAIN_ID);
        assert_eq!(tx.signature_type, SIGNATURE_TYPE_ED25519);
    }

    #[test]
    fn create_unbond_transaction_with_amount() {
        let keypair = KeyPair::generate();
        let tx = TransactionSigner::create_refiner_unbond(
            &keypair,
            FLAKES_PER_OPL,
            FLAKES_PER_OPL / 100,
            1,
            MAINNET_CHAIN_ID,
        );
        assert!(matches!(tx.action, TransactionAction::RefinerUnbond { amount } if amount == FLAKES_PER_OPL));
        assert_eq!(tx.nonce, 1);
        assert_eq!(tx.chain_id, MAINNET_CHAIN_ID);
        assert_eq!(tx.signature_type, SIGNATURE_TYPE_ED25519);
    }

    #[test]
    fn tx_id_includes_action() {
        let keypair = KeyPair::generate();
        let recipient = hash_to_object_id(b"recipient");
        let transfer = TransactionSigner::create_transfer(&keypair, recipient.clone(), 1000, 100, 0, MAINNET_CHAIN_ID);
        let bond = TransactionSigner::create_refiner_bond(&keypair, 1000, 100, 0, MAINNET_CHAIN_ID);
        // Same sender, same fee, same nonce, different action → different tx_id
        assert_ne!(transfer.tx_id, bond.tx_id);
    }

    #[test]
    fn tx_id_differs_across_chain_ids() {
        let keypair = KeyPair::generate();
        let recipient = hash_to_object_id(b"recipient");
        let other_chain_id: u64 = 2;
        let mainnet_tx = TransactionSigner::create_transfer(&keypair, recipient.clone(), 1000, 100, 0, MAINNET_CHAIN_ID);
        let other_tx = TransactionSigner::create_transfer(&keypair, recipient.clone(), 1000, 100, 0, other_chain_id);
        // Same sender, action, fee, nonce — but different chain_id → different tx_id
        assert_ne!(mainnet_tx.tx_id, other_tx.tx_id);
        assert_eq!(mainnet_tx.chain_id, MAINNET_CHAIN_ID);
        assert_eq!(other_tx.chain_id, other_chain_id);
    }
}