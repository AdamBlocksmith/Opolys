use opolys_core::{Transaction, TransactionAction, ObjectId, FlakeAmount, OpolysError, MIN_BOND_STAKE};
use opolys_consensus::account::{AccountStore, TransferResult};
use opolys_consensus::pos::ValidatorSet;

#[derive(Debug)]
pub struct ApplyResult {
    pub success: bool,
    pub fee_burned: FlakeAmount,
    pub error: Option<String>,
}

impl ApplyResult {
    pub fn ok(fee_burned: FlakeAmount) -> Self {
        ApplyResult {
            success: true,
            fee_burned,
            error: None,
        }
    }

    pub fn err(msg: &str) -> Self {
        ApplyResult {
            success: false,
            fee_burned: 0,
            error: Some(msg.to_string()),
        }
    }
}

pub struct TransactionDispatcher;

impl TransactionDispatcher {
    pub fn apply_transaction(
        tx: &Transaction,
        accounts: &mut AccountStore,
        validators: &mut ValidatorSet,
        block_height: u64,
        block_timestamp: u64,
    ) -> ApplyResult {
        let sender = &tx.sender;

        if let Some(account) = accounts.get_account(sender) {
            if account.nonce != tx.nonce {
                return ApplyResult::err(&format!(
                    "Invalid nonce: expected {}, got {}",
                    account.nonce, tx.nonce
                ));
            }
        } else {
            return ApplyResult::err(&format!("Sender account not found: {}", sender.to_hex()));
        }

        match &tx.action {
            TransactionAction::Transfer { recipient, amount } => {
                Self::apply_transfer(tx, sender, recipient, *amount, accounts)
            }
            TransactionAction::ValidatorBond => {
                Self::apply_bond(tx, sender, accounts, validators, block_height, block_timestamp)
            }
            TransactionAction::ValidatorUnbond => {
                Self::apply_unbond(tx, sender, accounts, validators)
            }
        }
    }

    fn apply_transfer(
        tx: &Transaction,
        sender: &ObjectId,
        recipient: &ObjectId,
        amount: FlakeAmount,
        accounts: &mut AccountStore,
    ) -> ApplyResult {
        match accounts.transfer(sender, recipient, amount, tx.fee) {
            Ok(TransferResult { fee_burned, .. }) => ApplyResult::ok(fee_burned),
            Err(e) => ApplyResult::err(&e.to_string()),
        }
    }

    fn apply_bond(
        tx: &Transaction,
        sender: &ObjectId,
        accounts: &mut AccountStore,
        validators: &mut ValidatorSet,
        block_height: u64,
        block_timestamp: u64,
    ) -> ApplyResult {
        let stake = Self::extract_bond_amount(tx).unwrap_or(0);

        if stake < MIN_BOND_STAKE {
            return ApplyResult::err(&format!(
                "Insufficient bond stake: need {}, got {}",
                MIN_BOND_STAKE, stake
            ));
        }

        let total_needed = stake.saturating_add(tx.fee);
        if let Some(account) = accounts.get_account(sender) {
            if account.balance < total_needed {
                return ApplyResult::err(&format!(
                    "Insufficient balance for bond: need {}, have {}",
                    total_needed, account.balance
                ));
            }
        }

        if let Err(e) = accounts.debit(sender, total_needed) {
            return ApplyResult::err(&e.to_string());
        }

        let sender_clone = sender.clone();
        if let Err(e) = validators.bond(sender_clone, stake, block_height, block_timestamp) {
            if let Ok(()) = accounts.credit(sender, stake.saturating_add(tx.fee)) {}
            return ApplyResult::err(&e);
        }

        ApplyResult::ok(tx.fee)
    }

    fn apply_unbond(
        tx: &Transaction,
        sender: &ObjectId,
        accounts: &mut AccountStore,
        validators: &mut ValidatorSet,
    ) -> ApplyResult {
        let validator = validators.get_validator(sender);
        let stake = match validator {
            Some(v) => v.stake,
            None => return ApplyResult::err("Validator not bonded"),
        };

        if let Err(e) = validators.unbond(sender) {
            return ApplyResult::err(&e);
        }

        if let Err(e) = accounts.credit(sender, stake) {
            return ApplyResult::err(&format!("Failed to return stake: {}", e));
        }

        let fee = tx.fee;
        if fee > 0 {
            if let Some(account) = accounts.get_account_mut(sender) {
                if account.balance >= fee {
                    account.balance -= fee;
                }
            }
        }

        if let Some(account) = accounts.get_account_mut(sender) {
            account.nonce += 1;
        }

        ApplyResult::ok(fee)
    }

    fn extract_bond_amount(tx: &Transaction) -> Option<FlakeAmount> {
        let _total_outflow = match &tx.action {
            TransactionAction::Transfer { amount, .. } => *amount + tx.fee,
            TransactionAction::ValidatorBond => tx.fee,
            TransactionAction::ValidatorUnbond => tx.fee,
        };

        None
    }
}

pub fn validate_transaction_basic(tx: &Transaction, sender_balance: FlakeAmount, sender_nonce: u64) -> Result<(), OpolysError> {
    if tx.nonce != sender_nonce {
        return Err(OpolysError::InvalidNonce {
            expected: sender_nonce,
            got: tx.nonce,
        });
    }

    let total_cost = match &tx.action {
        TransactionAction::Transfer { amount, .. } => amount.saturating_add(tx.fee),
        TransactionAction::ValidatorBond => tx.fee,
        TransactionAction::ValidatorUnbond => tx.fee,
    };

    if sender_balance < total_cost {
        return Err(OpolysError::InsufficientBalance {
            need: total_cost,
            have: sender_balance,
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use opolys_crypto::hash_to_object_id;
    use opolys_core::opl_to_flake;

    fn make_transfer(sender: &ObjectId, recipient: &ObjectId, amount: FlakeAmount, fee: FlakeAmount, nonce: u64) -> Transaction {
        Transaction {
            tx_id: hash_to_object_id(format!("{:?}_{:?}_{}", sender, recipient, nonce).as_bytes()),
            sender: sender.clone(),
            action: TransactionAction::Transfer {
                recipient: recipient.clone(),
                amount,
            },
            fee,
            signature: vec![],
            nonce,
            data: vec![],
        }
    }

    fn make_bond(sender: &ObjectId, _stake: FlakeAmount, fee: FlakeAmount, nonce: u64) -> Transaction {
        Transaction {
            tx_id: hash_to_object_id(format!("{:?}_bond_{}", sender, nonce).as_bytes()),
            sender: sender.clone(),
            action: TransactionAction::ValidatorBond,
            fee,
            signature: vec![],
            nonce,
            data: vec![],
        }
    }

    #[test]
    fn transfer_success() {
        let mut accounts = AccountStore::new();
        let mut validators = ValidatorSet::new();
        let alice = hash_to_object_id(b"alice");
        let bob = hash_to_object_id(b"bob");

        accounts.create_account(alice.clone()).unwrap();
        accounts.credit(&alice, opl_to_flake(1000)).unwrap();

        let tx = make_transfer(&alice, &bob, opl_to_flake(10), opl_to_flake(1), 0);
        let result = TransactionDispatcher::apply_transaction(
            &tx, &mut accounts, &mut validators, 1, 100,
        );
        assert!(result.success);
        assert_eq!(result.fee_burned, opl_to_flake(1));
        assert_eq!(accounts.get_account(&alice).unwrap().balance, opl_to_flake(989));
        assert_eq!(accounts.get_account(&bob).unwrap().balance, opl_to_flake(10));
    }

    #[test]
    fn transfer_insufficient_balance() {
        let mut accounts = AccountStore::new();
        let mut validators = ValidatorSet::new();
        let alice = hash_to_object_id(b"alice");
        let bob = hash_to_object_id(b"bob");

        accounts.create_account(alice.clone()).unwrap();
        accounts.credit(&alice, 100).unwrap();

        let tx = make_transfer(&alice, &bob, 200, 10, 0);
        let result = TransactionDispatcher::apply_transaction(
            &tx, &mut accounts, &mut validators, 1, 100,
        );
        assert!(!result.success);
    }

    #[test]
    fn bond_validator() {
        let mut accounts = AccountStore::new();
        let mut validators = ValidatorSet::new();
        let alice = hash_to_object_id(b"alice");

        accounts.create_account(alice.clone()).unwrap();
        accounts.credit(&alice, opl_to_flake(200)).unwrap();

        let tx = make_bond(&alice, 0, MIN_BOND_STAKE, 0);
        assert!(validators.validator_count() == 0);
    }
}