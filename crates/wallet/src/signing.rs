//! Transaction construction and signing for the Opolys blockchain.
//!
//! Provides `TransactionSigner`, a stateless utility that creates fully signed
//! `Transaction` objects ready for inclusion in a block. Each method serializes
//! the transaction data with Borsh, signs it with the sender's ed25519 key,
//! and computes a deterministic transaction ID via Blake3-256.
//!
//!.transactions are:
//! - **Transfer** — move OPL between accounts (fees are burned, not collected)
//! - **ValidatorBond** — lock OPL as stake to become a validator (min 100 OPL)
//! - **ValidatorUnbond** — release staked OPL back to the validator's balance

use opolys_core::{FlakeAmount, ObjectId, Transaction, TransactionAction};
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
            nonce,
            data: vec![],
        }
    }

    /// Create a signed validator bond transaction.
    ///
    /// Locks `amount` OPL (in flakes) as validator stake. The sender must have
    /// at least `amount + fee` in balance. `amount` must be `>=` MIN_BOND_STAKE
    /// (100 OPL). Only double-sign slashing exists in Opolys — no governance,
    /// no schedules, no fixed percentages.
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
            nonce,
            data: vec![],
        }
    }

/// Create a signed validator unbond transaction.
    ///
    /// Returns the full staked amount to the sender's balance. The fee is
    /// burned from the sender's balance. There is no lockup period —
    /// unbonding takes effect immediately upon block inclusion.
    pub fn create_validator_unbond(
        sender: &KeyPair,
        fee: FlakeAmount,
        nonce: u64,
    ) -> Transaction {
        let action = TransactionAction::ValidatorUnbond;
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
            nonce,
            data: vec![],
        }
    }

    /// Deterministic transaction ID derived from sender, fee, and nonce via Blake3-256.
    ///
    /// Ensures each transaction has a unique identifier without relying on
    /// randomness. Two transactions with the same sender, fee, and nonce
    /// will have the same ID — which is correct because the nonce prevents
    /// replay.
    fn compute_tx_id(
        sender: &ObjectId,
        _action: &TransactionAction,
        fee: FlakeAmount,
        nonce: u64,
    ) -> ObjectId {
        let mut data = sender.0.to_hex().as_bytes().to_vec();
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
        assert!(matches!(tx.action, TransactionAction::Transfer { .. }));
    }

    #[test]
    fn create_bond_transaction() {
        let keypair = KeyPair::generate();
        let bond_amount = 100 * FLAKES_PER_OPL; // 100 OPL
        let tx = TransactionSigner::create_validator_bond(
            &keypair,
            bond_amount,
            FLAKES_PER_OPL,
            0,
        );
        assert!(matches!(tx.action, TransactionAction::ValidatorBond { amount } if amount == bond_amount));
    }

    #[test]
    fn create_unbond_transaction() {
        let keypair = KeyPair::generate();
        let tx = TransactionSigner::create_validator_unbond(
            &keypair,
            FLAKES_PER_OPL / 100,
            1,
        );
        assert!(matches!(tx.action, TransactionAction::ValidatorUnbond));
        assert_eq!(tx.nonce, 1);
    }
}