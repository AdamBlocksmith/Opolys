//! Transaction construction and signing for the Opolys blockchain.
//!
//! Provides `TransactionSigner`, a stateless utility that creates fully signed
//! `Transaction` objects ready for inclusion in a block. Each method serializes
//! the transaction data with Borsh, signs it with the sender's ed25519 key,
//! and computes a deterministic transaction ID via Blake3-256.
//!
//! Transactions are:
//! - **Transfer** — move OPL between accounts (fees are burned, not collected)
//! - **ValidatorBond** — lock OPL as stake to become a validator (min 1 OPL per entry)
//! - **ValidatorUnbond** — release stake using FIFO order (oldest entries first)

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
    /// **burned** (permanently removed from supply), not collected by any validator.
    /// This is a core Opolys design choice: fees are market-driven and burned.
    pub fn create_transfer(
        sender: &KeyPair,
        recipient: ObjectId,
        amount: FlakeAmount,
        fee: FlakeAmount,
        nonce: u64,
    ) -> Transaction {
        let action = TransactionAction::Transfer { recipient, amount };
        let sender_id = sender.object_id().clone();

        let tx_id = Self::compute_tx_id(&sender_id, &action, fee, nonce);
        let unsigned_data = borsh::to_vec(&(sender_id.clone(), &action, fee, nonce)).unwrap_or_default();
        let signature = sender.sign(&unsigned_data);

        Transaction {
            tx_id,
            sender: sender_id,
            action,
            fee,
            signature,
            signature_type: SIGNATURE_TYPE_ED25519,
            nonce,
            data: vec![],
        }
    }

    /// Create a signed validator bond transaction.
    ///
    /// Locks `amount` OPL (in flakes) as validator stake. If the sender is
    /// already a validator, this creates a new bond entry (top-up) with its
    /// own seniority clock starting from zero. Each new entry must be at least
    /// `MIN_BOND_STAKE` (1 OPL).
    pub fn create_validator_bond(
        sender: &KeyPair,
        amount: FlakeAmount,
        fee: FlakeAmount,
        nonce: u64,
    ) -> Transaction {
        let action = TransactionAction::ValidatorBond { amount };
        let sender_id = sender.object_id().clone();

        let tx_id = Self::compute_tx_id(&sender_id, &action, fee, nonce);
        let unsigned_data = borsh::to_vec(&(sender_id.clone(), &action, fee, nonce)).unwrap_or_default();
        let signature = sender.sign(&unsigned_data);

        Transaction {
            tx_id,
            sender: sender_id,
            action,
            fee,
            signature,
            signature_type: SIGNATURE_TYPE_ED25519,
            nonce,
            data: vec![],
        }
    }

    /// Create a signed validator unbond transaction for FIFO amount-based unbonding.
    ///
    /// Unbonds `amount` Flakes from the validator's stake using FIFO order —
    /// the oldest entries are consumed first. If the amount exceeds an entry's
    /// stake, that entry is fully consumed and the remainder comes from the next
    /// oldest. After a UNBONDING_DELAY_BLOCKS delay, the unbonded stake is
    /// returned to the sender's wallet.
    pub fn create_validator_unbond(
        sender: &KeyPair,
        amount: FlakeAmount,
        fee: FlakeAmount,
        nonce: u64,
    ) -> Transaction {
        let action = TransactionAction::ValidatorUnbond { amount };
        let sender_id = sender.object_id().clone();

        let tx_id = Self::compute_tx_id(&sender_id, &action, fee, nonce);
        let unsigned_data = borsh::to_vec(&(sender_id.clone(), &action, fee, nonce)).unwrap_or_default();
        let signature = sender.sign(&unsigned_data);

        Transaction {
            tx_id,
            sender: sender_id,
            action,
            fee,
            signature,
            signature_type: SIGNATURE_TYPE_ED25519,
            nonce,
            data: vec![],
        }
    }

    /// Deterministic transaction ID derived from sender, action, fee, and nonce via Blake3-256.
    ///
    /// The action is included in the hash to prevent transaction ID collisions
    /// between different action types. Two transactions with the same sender,
    /// fee, and nonce but different actions will have different IDs.
    fn compute_tx_id(
        sender: &ObjectId,
        action: &TransactionAction,
        fee: FlakeAmount,
        nonce: u64,
    ) -> ObjectId {
        let mut data = sender.0.to_hex().as_bytes().to_vec();
        // Include the action in the hash to prevent ID collisions between
        // different action types with the same sender, fee, and nonce.
        data.extend_from_slice(borsh::to_vec(action).unwrap_or_default().as_slice());
        data.extend_from_slice(&fee.to_be_bytes());
        data.extend_from_slice(&nonce.to_be_bytes());
        hash_to_object_id(&data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opolys_core::FLAKES_PER_OPL;

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
        );
        assert_eq!(tx.nonce, 0);
        assert_eq!(tx.fee, FLAKES_PER_OPL / 10);
        assert_eq!(tx.signature_type, SIGNATURE_TYPE_ED25519);
        assert!(matches!(tx.action, TransactionAction::Transfer { .. }));
    }

    #[test]
    fn create_bond_transaction() {
        let keypair = KeyPair::generate();
        let bond_amount = FLAKES_PER_OPL;
        let tx = TransactionSigner::create_validator_bond(
            &keypair,
            bond_amount,
            FLAKES_PER_OPL,
            0,
        );
        assert!(matches!(tx.action, TransactionAction::ValidatorBond { amount } if amount == bond_amount));
        assert_eq!(tx.signature_type, SIGNATURE_TYPE_ED25519);
    }

    #[test]
    fn create_unbond_transaction_with_amount() {
        let keypair = KeyPair::generate();
        let tx = TransactionSigner::create_validator_unbond(
            &keypair,
            FLAKES_PER_OPL,
            FLAKES_PER_OPL / 100,
            1,
        );
        assert!(matches!(tx.action, TransactionAction::ValidatorUnbond { amount } if amount == FLAKES_PER_OPL));
        assert_eq!(tx.nonce, 1);
        assert_eq!(tx.signature_type, SIGNATURE_TYPE_ED25519);
    }

    #[test]
    fn tx_id_includes_action() {
        let keypair = KeyPair::generate();
        let recipient = hash_to_object_id(b"recipient");
        let transfer = TransactionSigner::create_transfer(&keypair, recipient.clone(), 1000, 100, 0);
        let bond = TransactionSigner::create_validator_bond(&keypair, 1000, 100, 0);
        // Same sender, same fee, same nonce, different action → different tx_id
        assert_ne!(transfer.tx_id, bond.tx_id);
    }
}