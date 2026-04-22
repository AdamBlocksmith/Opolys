use opolys_core::{FlakeAmount, ObjectId, Transaction, TransactionAction, FLAKES_PER_OPL};
use crate::key::KeyPair;
use opolys_crypto::hash_to_object_id;

pub struct TransactionSigner;

impl TransactionSigner {
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

    pub fn create_validator_bond(
        sender: &KeyPair,
        fee: FlakeAmount,
        nonce: u64,
    ) -> Transaction {
        let action = TransactionAction::ValidatorBond;
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

    fn compute_tx_id(
        sender: &ObjectId,
        action: &TransactionAction,
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
        let tx = TransactionSigner::create_validator_bond(
            &keypair,
            FLAKES_PER_OPL,
            0,
        );
        assert!(matches!(tx.action, TransactionAction::ValidatorBond));
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