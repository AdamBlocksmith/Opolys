//! Transaction dispatcher for the Opolys blockchain.
//!
//! This module provides the `TransactionDispatcher` — the single entry point
//! for executing transactions against chain state. Every transaction type
//! (Transfer, ValidatorBond, ValidatorUnbond) flows through `apply_transaction`,
//! which validates the sender's nonce, dispatches to a type-specific handler,
//! and returns an `ApplyResult`.
//!
//! # Fee model
//!
//! All transaction fees are **burned** (permanently removed from supply).
//! Validators do not collect fees — they earn from block rewards only.
//! This aligns with Opolys' model as decentralized digital gold: supply
//! expands via emission and contracts via fee burning, with no hard cap.

use opolys_core::{Transaction, TransactionAction, ObjectId, FlakeAmount, OpolysError, MIN_BOND_STAKE};
use opolys_consensus::account::{AccountStore, TransferResult};
use opolys_consensus::pos::ValidatorSet;

/// Result of applying a transaction to the chain state.
///
/// On success, `fee_burned` tracks how much OPL was permanently removed
/// from circulation. On failure, `error` describes why the transaction
/// was rejected (e.g. invalid nonce, insufficient balance).
#[derive(Debug)]
pub struct ApplyResult {
    /// Whether the transaction was successfully applied.
    pub success: bool,
    /// Amount of OPL (in flakes) burned as the transaction fee.
    /// Always 0 on failure.
    pub fee_burned: FlakeAmount,
    /// Human-readable error message if the transaction failed.
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

/// Stateless transaction dispatcher that applies transactions to chain state.
///
/// All methods are associated functions (no instance state) because transaction
/// execution is purely deterministic given its inputs. Every transaction fee is
/// burned (permanently removed from supply) — validators earn from block rewards,
/// not from fees.
pub struct TransactionDispatcher;

impl TransactionDispatcher {
    /// Apply a transaction against the current account and validator state.
    ///
    /// This is the single entry point for all transaction execution in Opolys.
    /// It first validates the sender's nonce, then dispatches to the appropriate
    /// handler based on `tx.action`:
    /// - `Transfer` → `apply_transfer`
    /// - `ValidatorBond` → `apply_bond`
    /// - `ValidatorUnbond` → `apply_unbond`
    ///
    /// Returns an `ApplyResult` indicating success or failure, along with the
    /// fee amount that was burned.
    pub fn apply_transaction(
        tx: &Transaction,
        accounts: &mut AccountStore,
        validators: &mut ValidatorSet,
        block_height: u64,
        block_timestamp: u64,
    ) -> ApplyResult {
        let sender = &tx.sender;

        // Verify the sender exists and nonce matches
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
            TransactionAction::ValidatorBond { amount } => {
                Self::apply_bond(tx, sender, *amount, accounts, validators, block_height, block_timestamp)
            }
            TransactionAction::ValidatorUnbond => {
                Self::apply_unbond(tx, sender, accounts, validators)
            }
        }
    }

    /// Transfer OPL from sender to recipient.
    ///
    /// Delegates to `AccountStore::transfer`, which debits `amount + fee` from
    /// the sender, credits `amount` to the recipient, and burns `fee`. The
    /// sender's nonce is incremented by the account store on success.
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

    /// Bond OPL as validator stake.
    ///
    /// The sender's balance is debited by `stake + fee`, where `stake` becomes
    /// locked validator stake and `fee` is burned. Requires `stake >= MIN_BOND_STAKE`
    /// (100 OPL). If the validator set rejects the bond (e.g. duplicate), the
    /// full debit is refunded to the sender.
    fn apply_bond(
        tx: &Transaction,
        sender: &ObjectId,
        stake: FlakeAmount,
        accounts: &mut AccountStore,
        validators: &mut ValidatorSet,
        block_height: u64,
        block_timestamp: u64,
    ) -> ApplyResult {
        // Minimum stake check — must be >= MIN_BOND_STAKE (100 OPL)
        if stake < MIN_BOND_STAKE {
            return ApplyResult::err(&format!(
                "Insufficient bond stake: need {}, got {}",
                MIN_BOND_STAKE, stake
            ));
        }

        // Verify total outflow (stake + fee) doesn't exceed balance
        let total_needed = stake.saturating_add(tx.fee);
        if let Some(account) = accounts.get_account(sender) {
            if account.balance < total_needed {
                return ApplyResult::err(&format!(
                    "Insufficient balance for bond: need {}, have {}",
                    total_needed, account.balance
                ));
            }
        }

        // Debit the sender's account for stake + fee
        if let Err(e) = accounts.debit(sender, total_needed) {
            return ApplyResult::err(&e.to_string());
        }

        // Register the validator with the staked amount
        let sender_clone = sender.clone();
        if let Err(e) = validators.bond(sender_clone, stake, block_height, block_timestamp) {
            // Refund on failure
            if let Ok(()) = accounts.credit(sender, total_needed) {}
            return ApplyResult::err(&e);
        }

        // Fee is burned (already debited from sender, not credited to anyone)
        ApplyResult::ok(tx.fee)
    }

    /// Unbond a validator, returning all staked OPL to the sender's balance.
    ///
    /// The full stake is credited back. The transaction fee is then burned
    /// (debited) from the sender's balance. The sender's nonce is incremented
    /// manually here (unlike transfers, the account store does not handle it).
    /// There is no lockup period — unbonding is immediate.
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

        // Remove the validator from the set
        if let Err(e) = validators.unbond(sender) {
            return ApplyResult::err(&e);
        }

        // Return the full stake to the sender
        if let Err(e) = accounts.credit(sender, stake) {
            return ApplyResult::err(&format!("Failed to return stake: {}", e));
        }

        // Burn the fee from the sender's balance
        let fee = tx.fee;
        if fee > 0 {
            if let Some(account) = accounts.get_account_mut(sender) {
                if account.balance >= fee {
                    account.balance -= fee;
                }
            }
        }

        // Increment the sender's nonce
        if let Some(account) = accounts.get_account_mut(sender) {
            account.nonce += 1;
        }

        ApplyResult::ok(fee)
    }
}

/// Basic pre-validation of a transaction before it enters the mempool.
///
/// Checks nonce correctness and balance sufficiency for the transaction's
/// total cost (amount + fee for transfers and bonds, fee only for unbonding).
/// Does **not** verify the signature — that happens during block execution.
///
/// This is a fast check to reject obviously invalid transactions before they
/// consume mempool space or network bandwidth.
pub fn validate_transaction_basic(tx: &Transaction, sender_balance: FlakeAmount, sender_nonce: u64) -> Result<(), OpolysError> {
    if tx.nonce != sender_nonce {
        return Err(OpolysError::InvalidNonce {
            expected: sender_nonce,
            got: tx.nonce,
        });
    }

    // Total cost depends on the transaction type
    let total_cost = match &tx.action {
        TransactionAction::Transfer { amount, .. } => amount.saturating_add(tx.fee),
        TransactionAction::ValidatorBond { amount } => amount.saturating_add(tx.fee),
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

    fn make_bond(sender: &ObjectId, amount: FlakeAmount, fee: FlakeAmount, nonce: u64) -> Transaction {
        Transaction {
            tx_id: hash_to_object_id(format!("{:?}_bond_{}", sender, nonce).as_bytes()),
            sender: sender.clone(),
            action: TransactionAction::ValidatorBond { amount },
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
        assert!(result.success, "Transfer should succeed: {:?}", result.error);
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

    /// Bond validator with sufficient stake — should succeed.
    /// Alice bonds 100 OPL (MIN_BOND_STAKE) with a 1 OPL fee.
    #[test]
    fn bond_validator_success() {
        let mut accounts = AccountStore::new();
        let mut validators = ValidatorSet::new();
        let alice = hash_to_object_id(b"alice");

        accounts.create_account(alice.clone()).unwrap();
        accounts.credit(&alice, opl_to_flake(200)).unwrap();

        let bond_amount = MIN_BOND_STAKE; // 100 OPL
        let fee = opl_to_flake(1);
        let tx = make_bond(&alice, bond_amount, fee, 0);
        let result = TransactionDispatcher::apply_transaction(
            &tx, &mut accounts, &mut validators, 1, 100,
        );
        assert!(result.success, "Bond should succeed: {:?}", result.error);
        assert_eq!(validators.validator_count(), 1);
        // Alice's balance: 200 - 100 (stake) - 1 (fee) = 99 OPL
        assert_eq!(accounts.get_account(&alice).unwrap().balance, opl_to_flake(99));
        // Validator should have 100 OPL staked
        assert_eq!(validators.get_validator(&alice).unwrap().stake, bond_amount);
    }

    /// Bond validator with insufficient stake — should fail.
    #[test]
    fn bond_validator_below_minimum() {
        let mut accounts = AccountStore::new();
        let mut validators = ValidatorSet::new();
        let alice = hash_to_object_id(b"alice");

        accounts.create_account(alice.clone()).unwrap();
        accounts.credit(&alice, opl_to_flake(200)).unwrap();

        let too_low = opl_to_flake(50); // Below MIN_BOND_STAKE
        let tx = make_bond(&alice, too_low, opl_to_flake(1), 0);
        let result = TransactionDispatcher::apply_transaction(
            &tx, &mut accounts, &mut validators, 1, 100,
        );
        assert!(!result.success);
    }

    #[test]
    fn validate_transaction_basic_transfer() {
        let alice = hash_to_object_id(b"alice");
        let bob = hash_to_object_id(b"bob");
        let tx = make_transfer(&alice, &bob, 100, 10, 0);
        assert!(validate_transaction_basic(&tx, 200, 0).is_ok());
        assert!(validate_transaction_basic(&tx, 50, 0).is_err());
    }

    #[test]
    fn validate_transaction_basic_bond() {
        let alice = hash_to_object_id(b"alice");
        let tx = make_bond(&alice, MIN_BOND_STAKE, opl_to_flake(1), 0);
        // Total cost = MIN_BOND_STAKE + 1 OPL fee
        assert!(validate_transaction_basic(&tx, MIN_BOND_STAKE + opl_to_flake(1), 0).is_ok());
        assert!(validate_transaction_basic(&tx, MIN_BOND_STAKE, 0).is_err());
    }
}