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
//!
//! # Per-entry validator bonds with FIFO unbonding
//!
//! Validators can hold multiple bond entries, each with its own stake amount
//! and seniority clock. `ValidatorBond` creates a new entry (or the first
//! one if the validator doesn't exist yet). `ValidatorUnbond { amount }`
//! unbonds the specified amount using FIFO order — oldest entries consumed
//! first. If the amount exceeds an entry's stake, that entry is fully
//! consumed and the remainder comes from the next oldest. Split entries
//! keep their original `bonded_at_timestamp`. Invalid amounts (below
//! MIN_FEE floor or exceeding total stake) result in an error.

use opolys_core::{Transaction, TransactionAction, ObjectId, FlakeAmount, OpolysError, MIN_BOND_STAKE, MIN_FEE, SIGNATURE_TYPE_ED25519};
use opolys_consensus::account::{AccountStore, TransferResult};
use opolys_consensus::pos::ValidatorSet;
use opolys_crypto::hash_to_object_id;

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
    /// It first validates the sender's nonce and minimum fee, then dispatches
    /// to the appropriate handler based on `tx.action`:
    /// - `Transfer` → `apply_transfer`
    /// - `ValidatorBond` → `apply_bond`
    /// - `ValidatorUnbond { amount }` → `apply_unbond`
    ///
    /// `expected_chain_id` must match the network the node is running on
    /// (MAINNET_CHAIN_ID=1, TESTNET_CHAIN_ID=2). Transactions with a mismatched
    /// chain_id are rejected to prevent cross-chain replay attacks.
    ///
    /// Returns an `ApplyResult` indicating success or failure, along with the
    /// fee amount that was burned.
    pub fn apply_transaction(
        tx: &Transaction,
        accounts: &mut AccountStore,
        validators: &mut ValidatorSet,
        block_height: u64,
        block_timestamp: u64,
        expected_chain_id: u64,
    ) -> ApplyResult {
        // Verify transaction ID integrity, signature, and chain ID
        if let Err(e) = verify_transaction(tx, expected_chain_id) {
            return ApplyResult::err(&e.to_string());
        }

        // Verify minimum fee floor
        if tx.fee < MIN_FEE {
            return ApplyResult::err(&format!(
                "Fee below minimum: need at least {} flakes, got {}",
                MIN_FEE, tx.fee
            ));
        }

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

        let result = match &tx.action {
            TransactionAction::Transfer { recipient, amount } => {
                Self::apply_transfer(tx, sender, recipient, *amount, accounts)
            }
            TransactionAction::ValidatorBond { amount } => {
                Self::apply_bond(tx, sender, *amount, accounts, validators, block_height, block_timestamp)
            }
            TransactionAction::ValidatorUnbond { amount } => {
                Self::apply_unbond(tx, sender, *amount, accounts, validators, block_height)
            }
        };

        // After a successful transaction, store the sender's public key in
        // their account. The first real transaction from a pre-funded account
        // registers the key; subsequent transactions update it. This enables
        // future signature verification without a separate key registry.
        if result.success && !tx.public_key.is_empty() {
            if let Some(account) = accounts.get_account_mut(sender) {
                account.public_key = Some(tx.public_key.clone());
            }
        }

        result
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
    /// If the sender is already a validator, this creates a new bond entry
    /// (top-up) with its own seniority clock starting from zero, or merges
    /// with an existing entry at the same timestamp. Each new entry must be
    /// at least `MIN_BOND_STAKE` (1 OPL).
    ///
    /// If the sender is not yet a validator, this creates a new validator with
    /// this as their first bond entry (status: Bonding).
    ///
    /// The sender's balance is debited by `stake + fee`, where `stake` becomes
    /// locked validator stake and `fee` is burned. If the bond fails (e.g.
    /// insufficient balance), the debit is refunded.
    fn apply_bond(
        tx: &Transaction,
        sender: &ObjectId,
        stake: FlakeAmount,
        accounts: &mut AccountStore,
        validators: &mut ValidatorSet,
        block_height: u64,
        block_timestamp: u64,
    ) -> ApplyResult {
        // Minimum stake per new entry — must be >= MIN_BOND_STAKE (1 OPL)
        if stake < MIN_BOND_STAKE {
            return ApplyResult::err(&format!(
                "Insufficient bond stake per entry: need {}, got {}",
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

        // Register the bond (creates new entry or merges with same-timestamp entry)
        let sender_clone = sender.clone();
        if let Err(e) = validators.bond(sender_clone, stake, block_height, block_timestamp) {
            // Refund on failure
            if let Ok(()) = accounts.credit(sender, total_needed) {}
            return ApplyResult::err(&e);
        }

        // Increment the sender's nonce to prevent replay
        if let Some(account) = accounts.get_account_mut(sender) {
            account.nonce += 1;
        }

        // Fee is burned (already debited from sender, not credited to anyone)
        ApplyResult::ok(tx.fee)
    }

    /// Unbond `amount` Flakes from the validator using FIFO order.
    ///
    /// The oldest entries are consumed first. If the amount exceeds an entry's
    /// stake, that entry is fully consumed and the remainder comes from the
    /// next oldest. The unbonded stake enters the unbonding queue and is
    /// returned after `UNBONDING_DELAY_BLOCKS` (1,024 blocks = one epoch).
    ///
    /// The transaction fee is burned from the sender's balance immediately.
    /// If the validator has insufficient stake, the transaction fails with
    /// no fee burn and no nonce advance.
    ///
    fn apply_unbond(
        tx: &Transaction,
        sender: &ObjectId,
        amount: FlakeAmount,
        accounts: &mut AccountStore,
        validators: &mut ValidatorSet,
        block_height: u64,
    ) -> ApplyResult {
        // Check that the validator exists
        if validators.get_validator(sender).is_none() {
            return ApplyResult::err("Validator not bonded");
        }

        // Perform FIFO unbond — oldest entries consumed first, queued for delayed return
        let unbonded = match validators.unbond_amount(sender, amount, block_height) {
            Ok(stake) => stake,
            Err(e) => return ApplyResult::err(&e),
        };

        if unbonded == 0 {
            return ApplyResult::err("No stake unbonded");
        }

        // The unbonded stake enters the unbonding queue and will be returned
        // after UNBONDING_DELAY_BLOCKS (1,024 blocks). It is NOT credited
        // to the sender's account immediately.

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
/// Checks nonce correctness, minimum fee, and balance sufficiency for
/// the transaction's total cost (amount + fee for transfers and bonds,
/// fee only for unbonding). Does **not** verify the signature — that
/// happens during block execution.
///
/// This is a fast check to reject obviously invalid transactions before
/// they consume mempool space or network bandwidth.
pub fn validate_transaction_basic(tx: &Transaction, sender_balance: FlakeAmount, sender_nonce: u64) -> Result<(), OpolysError> {
    // Check minimum fee
    if tx.fee < MIN_FEE {
        return Err(OpolysError::InvalidParams(format!(
            "Fee below minimum: need at least {} flakes, got {}",
            MIN_FEE, tx.fee
        )));
    }

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
        TransactionAction::ValidatorUnbond { .. } => tx.fee,
    };

    if sender_balance < total_cost {
        return Err(OpolysError::InsufficientBalance {
            need: total_cost,
            have: sender_balance,
        });
    }

    Ok(())
}

/// Verify a transaction's cryptographic signature, tx_id integrity, and chain ID.
///
/// This is the full verification that should be performed before a transaction
/// enters a block. It checks:
///
/// 1. **chain_id**: Must equal `expected_chain_id`. Prevents cross-chain replay attacks.
/// 2. **tx_id integrity**: Recomputes the transaction ID from (sender, action, fee, nonce, chain_id)
///    and verifies it matches the declared `tx.tx_id`.
/// 3. **signature_type**: Must be ed25519 (type 0), the only supported type.
/// 4. **public_key length**: Must be exactly 32 bytes. Empty keys are rejected.
/// 5. **SenderId binding**: `Blake3(public_key)` must equal `sender` ObjectId.
///    This proves the public key belongs to the claimed sender.
/// 6. **ed25519 signature**: The signature must be valid over the Borsh-serialized
///    (sender, action, fee, nonce, chain_id) tuple using the provided public key.
///
/// Note: This function does NOT check nonce, balance, or fee minimums. Those are
/// checked by `validate_transaction_basic` and `apply_transaction`.
pub fn verify_transaction(tx: &Transaction, expected_chain_id: u64) -> Result<(), OpolysError> {
    // 1. Verify chain ID to prevent cross-chain replay attacks
    if tx.chain_id != expected_chain_id {
        return Err(OpolysError::InvalidTransaction(format!(
            "Transaction chain_id {} does not match network chain_id {}",
            tx.chain_id, expected_chain_id
        )));
    }

    // 2. Verify tx_id integrity
    let expected_tx_id = compute_tx_id(&tx.sender, &tx.action, tx.fee, tx.nonce, tx.chain_id);
    if tx.tx_id != expected_tx_id {
        return Err(OpolysError::InvalidTransaction(format!(
            "Transaction ID mismatch: expected {}, got {}",
            expected_tx_id.to_hex(),
            tx.tx_id.to_hex()
        )));
    }

    // 3. Verify signature type
    if tx.signature_type != SIGNATURE_TYPE_ED25519 {
        return Err(OpolysError::InvalidTransaction(format!(
            "Unsupported signature type: {} (only ed25519 = 0 is supported)",
            tx.signature_type
        )));
    }

    // 4. Verify public key length (ed25519 = 32 bytes) — must not be empty
    if tx.public_key.len() != 32 {
        return Err(OpolysError::InvalidTransaction(format!(
            "Invalid public key length: {} bytes (expected 32 for ed25519)",
            tx.public_key.len()
        )));
    }

    // 5. Verify Blake3(public_key) == sender ObjectId binding
    let pk_bytes: [u8; 32] = match tx.public_key.as_slice().try_into() {
        Ok(b) => b,
        Err(_) => return Err(OpolysError::InvalidTransaction(
            "Public key conversion failed".to_string()
        )),
    };
    let derived_object_id = opolys_crypto::ed25519_public_key_to_object_id(&pk_bytes);
    if tx.sender != derived_object_id {
        return Err(OpolysError::InvalidTransaction(format!(
            "Public key does not match sender: Blake3(pk)={}, sender={}",
            derived_object_id.to_hex(),
            tx.sender.to_hex()
        )));
    }

    // 6. Verify ed25519 signature over the Borsh-serialized (sender, action, fee, nonce, chain_id)
    let unsigned_data = borsh::to_vec(&(tx.sender.clone(), &tx.action, tx.fee, tx.nonce, tx.chain_id))
        .map_err(|e| OpolysError::InvalidTransaction(format!("Failed to serialize tx data: {}", e)))?;

    if !opolys_crypto::verify_ed25519(&tx.public_key, &unsigned_data, &tx.signature) {
        return Err(OpolysError::InvalidSignature);
    }

    Ok(())
}

/// Compute the expected transaction ID from the transaction fields.
///
/// Matches the wallet's `TransactionSigner::compute_tx_id` function exactly:
/// Blake3-256(sender_hex || borsh(action) || fee_bytes || nonce_bytes || chain_id_bytes)
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

#[cfg(test)]
mod tests {
    use super::*;
    use opolys_core::{opl_to_flake, EPOCH, MAINNET_CHAIN_ID};
    use opolys_crypto::hash_to_object_id;
    use ed25519_dalek::{SigningKey, Signer};

    /// Deterministic test keypair from a single seed byte.
    fn test_keypair(seed: u8) -> (SigningKey, ObjectId, Vec<u8>) {
        let signing_key = SigningKey::from_bytes(&[seed; 32]);
        let pk = signing_key.verifying_key();
        let pk_bytes = pk.as_bytes().to_vec();
        let id = opolys_crypto::ed25519_public_key_to_object_id(pk.as_bytes());
        (signing_key, id, pk_bytes)
    }

    fn signed_transfer(sk: &SigningKey, sender: &ObjectId, pk: Vec<u8>, to: &ObjectId, amount: FlakeAmount, fee: FlakeAmount, nonce: u64) -> Transaction {
        let action = TransactionAction::Transfer { recipient: to.clone(), amount };
        let tx_id = compute_tx_id(sender, &action, fee, nonce, MAINNET_CHAIN_ID);
        let msg = borsh::to_vec(&(sender.clone(), &action, fee, nonce, MAINNET_CHAIN_ID)).unwrap();
        Transaction {
            tx_id, sender: sender.clone(), action, fee,
            signature: sk.sign(&msg).to_bytes().to_vec(),
            signature_type: SIGNATURE_TYPE_ED25519, nonce,
            chain_id: MAINNET_CHAIN_ID, data: vec![], public_key: pk,
        }
    }

    fn signed_bond(sk: &SigningKey, sender: &ObjectId, pk: Vec<u8>, amount: FlakeAmount, fee: FlakeAmount, nonce: u64) -> Transaction {
        let action = TransactionAction::ValidatorBond { amount };
        let tx_id = compute_tx_id(sender, &action, fee, nonce, MAINNET_CHAIN_ID);
        let msg = borsh::to_vec(&(sender.clone(), &action, fee, nonce, MAINNET_CHAIN_ID)).unwrap();
        Transaction {
            tx_id, sender: sender.clone(), action, fee,
            signature: sk.sign(&msg).to_bytes().to_vec(),
            signature_type: SIGNATURE_TYPE_ED25519, nonce,
            chain_id: MAINNET_CHAIN_ID, data: vec![], public_key: pk,
        }
    }

    fn signed_unbond(sk: &SigningKey, sender: &ObjectId, pk: Vec<u8>, amount: FlakeAmount, fee: FlakeAmount, nonce: u64) -> Transaction {
        let action = TransactionAction::ValidatorUnbond { amount };
        let tx_id = compute_tx_id(sender, &action, fee, nonce, MAINNET_CHAIN_ID);
        let msg = borsh::to_vec(&(sender.clone(), &action, fee, nonce, MAINNET_CHAIN_ID)).unwrap();
        Transaction {
            tx_id, sender: sender.clone(), action, fee,
            signature: sk.sign(&msg).to_bytes().to_vec(),
            signature_type: SIGNATURE_TYPE_ED25519, nonce,
            chain_id: MAINNET_CHAIN_ID, data: vec![], public_key: pk,
        }
    }

    /// Unsigned helper used only for validate_transaction_basic tests (no sig check).
    fn unsigned_transfer(sender: &ObjectId, recipient: &ObjectId, amount: FlakeAmount, fee: FlakeAmount, nonce: u64) -> Transaction {
        let action = TransactionAction::Transfer { recipient: recipient.clone(), amount };
        let tx_id = compute_tx_id(sender, &action, fee, nonce, MAINNET_CHAIN_ID);
        Transaction {
            tx_id, sender: sender.clone(), action, fee,
            signature: vec![], signature_type: SIGNATURE_TYPE_ED25519, nonce,
            chain_id: MAINNET_CHAIN_ID, data: vec![], public_key: vec![],
        }
    }

    /// Unsigned helper used only for validate_transaction_basic tests (no sig check).
    fn unsigned_bond(sender: &ObjectId, amount: FlakeAmount, fee: FlakeAmount, nonce: u64) -> Transaction {
        let action = TransactionAction::ValidatorBond { amount };
        let tx_id = compute_tx_id(sender, &action, fee, nonce, MAINNET_CHAIN_ID);
        Transaction {
            tx_id, sender: sender.clone(), action, fee,
            signature: vec![], signature_type: SIGNATURE_TYPE_ED25519, nonce,
            chain_id: MAINNET_CHAIN_ID, data: vec![], public_key: vec![],
        }
    }

    #[test]
    fn transfer_success() {
        let mut accounts = AccountStore::new();
        let mut validators = ValidatorSet::new();
        let (sk, alice, pk) = test_keypair(1);
        let (_, bob, _) = test_keypair(2);

        accounts.create_account(alice.clone()).unwrap();
        accounts.credit(&alice, opl_to_flake(1000)).unwrap();

        let tx = signed_transfer(&sk, &alice, pk, &bob, opl_to_flake(10), opl_to_flake(1), 0);
        let result = TransactionDispatcher::apply_transaction(
            &tx, &mut accounts, &mut validators, 1, 100, MAINNET_CHAIN_ID,
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
        let (sk, alice, pk) = test_keypair(1);
        let (_, bob, _) = test_keypair(2);

        accounts.create_account(alice.clone()).unwrap();
        accounts.credit(&alice, 100).unwrap();

        let tx = signed_transfer(&sk, &alice, pk, &bob, 200, 10, 0);
        let result = TransactionDispatcher::apply_transaction(
            &tx, &mut accounts, &mut validators, 1, 100, MAINNET_CHAIN_ID,
        );
        assert!(!result.success);
    }

    /// Bond validator with sufficient stake — should succeed.
    /// Alice bonds 1 OPL (MIN_BOND_STAKE) with a 1 OPL fee.
    #[test]
    fn bond_validator_success() {
        let mut accounts = AccountStore::new();
        let mut validators = ValidatorSet::new();
        let (sk, alice, pk) = test_keypair(1);

        accounts.create_account(alice.clone()).unwrap();
        accounts.credit(&alice, opl_to_flake(200)).unwrap();

        let tx = signed_bond(&sk, &alice, pk, MIN_BOND_STAKE, opl_to_flake(1), 0);
        let result = TransactionDispatcher::apply_transaction(
            &tx, &mut accounts, &mut validators, 1, 100, MAINNET_CHAIN_ID,
        );
        assert!(result.success, "Bond should succeed: {:?}", result.error);
        assert_eq!(validators.validator_count(), 1);
        // Alice's balance: 200 - 1 (stake) - 1 (fee) = 198 OPL
        assert_eq!(accounts.get_account(&alice).unwrap().balance, opl_to_flake(198));
    }

    /// Bond validator with insufficient stake — should fail.
    #[test]
    fn bond_validator_below_minimum() {
        let mut accounts = AccountStore::new();
        let mut validators = ValidatorSet::new();
        let (sk, alice, pk) = test_keypair(1);

        accounts.create_account(alice.clone()).unwrap();
        accounts.credit(&alice, opl_to_flake(200)).unwrap();

        let too_low = 50; // Below MIN_BOND_STAKE (1 OPL = 1,000,000 flakes)
        let tx = signed_bond(&sk, &alice, pk, too_low, opl_to_flake(1), 0);
        let result = TransactionDispatcher::apply_transaction(
            &tx, &mut accounts, &mut validators, 1, 100, MAINNET_CHAIN_ID,
        );
        assert!(!result.success);
    }

    /// Top-up: existing validator bonds again, creating a second entry.
    #[test]
    fn bond_top_up_creates_new_entry() {
        let mut accounts = AccountStore::new();
        let mut validators = ValidatorSet::new();
        let (sk, alice, pk) = test_keypair(1);

        accounts.create_account(alice.clone()).unwrap();
        accounts.credit(&alice, opl_to_flake(500)).unwrap();

        // First bond: 1 OPL
        let tx1 = signed_bond(&sk, &alice, pk.clone(), MIN_BOND_STAKE, opl_to_flake(1), 0);
        let result1 = TransactionDispatcher::apply_transaction(
            &tx1, &mut accounts, &mut validators, 1, 100, MAINNET_CHAIN_ID,
        );
        assert!(result1.success);

        // Second bond (top-up): 2 OPL
        let tx2 = signed_bond(&sk, &alice, pk, MIN_BOND_STAKE * 2, opl_to_flake(1), 1);
        let result2 = TransactionDispatcher::apply_transaction(
            &tx2, &mut accounts, &mut validators, 2, 200, MAINNET_CHAIN_ID,
        );
        assert!(result2.success, "Top-up bond should succeed: {:?}", result2.error);

        let v = validators.get_validator(&alice).unwrap();
        assert_eq!(v.entries.len(), 2);
        assert_eq!(v.total_stake(), MIN_BOND_STAKE * 3);
        assert_eq!(validators.validator_count(), 1);
    }

    /// Unbond using FIFO — oldest entries consumed first.
    #[test]
    fn unbond_fifo_partial() {
        let mut accounts = AccountStore::new();
        let mut validators = ValidatorSet::new();
        let (sk, alice, pk) = test_keypair(1);

        accounts.create_account(alice.clone()).unwrap();
        accounts.credit(&alice, opl_to_flake(500)).unwrap();

        // Bond 1 OPL at t=100
        let tx1 = signed_bond(&sk, &alice, pk.clone(), MIN_BOND_STAKE, opl_to_flake(1), 0);
        let r1 = TransactionDispatcher::apply_transaction(&tx1, &mut accounts, &mut validators, 1, 100, MAINNET_CHAIN_ID);
        assert!(r1.success, "First bond should succeed: {:?}", r1.error);

        // Bond 2 OPL at t=1000
        let tx2 = signed_bond(&sk, &alice, pk.clone(), MIN_BOND_STAKE * 2, opl_to_flake(1), 1);
        let r2 = TransactionDispatcher::apply_transaction(&tx2, &mut accounts, &mut validators, 2, 1000, MAINNET_CHAIN_ID);
        assert!(r2.success, "Second bond (top-up) should succeed: {:?}", r2.error);

        // Alice balance: 500 - 1 - 1 - 2 - 1 = 495 OPL
        assert_eq!(accounts.get_account(&alice).unwrap().balance, opl_to_flake(495));

        // Unbond 1.5 OPL — consumes first entry (1 OPL) + 0.5 OPL from second entry
        let tx3 = signed_unbond(&sk, &alice, pk, MIN_BOND_STAKE + MIN_BOND_STAKE / 2, opl_to_flake(1), 2);
        let result = TransactionDispatcher::apply_transaction(
            &tx3, &mut accounts, &mut validators, 3, 300, MAINNET_CHAIN_ID,
        );
        assert!(result.success, "Unbond should succeed: {:?}", result.error);

        let v = validators.get_validator(&alice).unwrap();
        assert_eq!(v.entries.len(), 1);
        // Remaining: 2 - 0.5 = 1.5 OPL
        assert_eq!(v.total_stake(), MIN_BOND_STAKE * 3 / 2);

        // Unbonded stake goes into the unbonding queue, not immediately returned
        assert_eq!(validators.unbonding_queue.len(), 1);
        assert_eq!(validators.unbonding_queue[0].amount, MIN_BOND_STAKE + MIN_BOND_STAKE / 2);
        assert_eq!(validators.unbonding_queue[0].matures_at, 3 + EPOCH as u64);

        // Alice's balance: 500 - 1 - 1 (bond1) - 2 - 1 (bond2) - 1 (unbond fee) = 494 OPL
        assert_eq!(accounts.get_account(&alice).unwrap().balance, opl_to_flake(494));
    }

    /// Unbond more than total stake — unbonds all available.
    #[test]
    fn unbond_more_than_stake() {
        let mut accounts = AccountStore::new();
        let mut validators = ValidatorSet::new();
        let (sk, alice, pk) = test_keypair(1);

        accounts.create_account(alice.clone()).unwrap();
        accounts.credit(&alice, opl_to_flake(200)).unwrap();

        let tx1 = signed_bond(&sk, &alice, pk.clone(), MIN_BOND_STAKE, opl_to_flake(1), 0);
        let r = TransactionDispatcher::apply_transaction(&tx1, &mut accounts, &mut validators, 1, 100, MAINNET_CHAIN_ID);
        assert!(r.success, "Bond should succeed: {:?}", r.error);

        // Try to unbond 10x the total stake
        let tx2 = signed_unbond(&sk, &alice, pk, MIN_BOND_STAKE * 10, opl_to_flake(1), 1);
        let result = TransactionDispatcher::apply_transaction(
            &tx2, &mut accounts, &mut validators, 2, 200, MAINNET_CHAIN_ID,
        );
        assert!(result.success);
        // Validator removed because all stake unbonded
        assert_eq!(validators.validator_count(), 0);
        // But the unbonded stake is in the queue, not immediately credited
        assert_eq!(validators.unbonding_queue.len(), 1);
    }

    /// Unbond a non-existent validator — should fail.
    #[test]
    fn unbond_nonexistent_validator() {
        let mut accounts = AccountStore::new();
        let mut validators = ValidatorSet::new();
        let (sk, alice, pk) = test_keypair(1);

        // Alice has no account and is not a validator
        let tx = signed_unbond(&sk, &alice, pk, MIN_BOND_STAKE, opl_to_flake(1), 0);
        let result = TransactionDispatcher::apply_transaction(
            &tx, &mut accounts, &mut validators, 1, 100, MAINNET_CHAIN_ID,
        );
        assert!(!result.success);
    }

    #[test]
    fn validate_transaction_basic_transfer() {
        let alice = hash_to_object_id(b"alice");
        let bob = hash_to_object_id(b"bob");
        let tx = unsigned_transfer(&alice, &bob, 100, 10, 0);
        assert!(validate_transaction_basic(&tx, 200, 0).is_ok());
        assert!(validate_transaction_basic(&tx, 50, 0).is_err());
    }

    #[test]
    fn validate_transaction_basic_bond() {
        let alice = hash_to_object_id(b"alice");
        let tx = unsigned_bond(&alice, MIN_BOND_STAKE, opl_to_flake(1), 0);
        // Total cost = MIN_BOND_STAKE + 1 OPL fee
        assert!(validate_transaction_basic(&tx, MIN_BOND_STAKE + opl_to_flake(1), 0).is_ok());
        assert!(validate_transaction_basic(&tx, MIN_BOND_STAKE, 0).is_err());
    }

    /// Bond transaction should store the sender's public key in their account.
    #[test]
    fn bond_stores_public_key_in_account() {
        let mut accounts = AccountStore::new();
        let mut validators = ValidatorSet::new();
        let (sk, alice, pk) = test_keypair(42);

        accounts.create_account(alice.clone()).unwrap();
        accounts.credit(&alice, opl_to_flake(200)).unwrap();

        let tx = signed_bond(&sk, &alice, pk.clone(), MIN_BOND_STAKE, opl_to_flake(1), 0);
        let result = TransactionDispatcher::apply_transaction(
            &tx, &mut accounts, &mut validators, 1, 100, MAINNET_CHAIN_ID,
        );
        assert!(result.success, "Bond should succeed: {:?}", result.error);

        let account = accounts.get_account(&alice).unwrap();
        assert!(account.public_key.is_some(), "Public key should be stored after bond");
        assert_eq!(account.public_key.as_ref().unwrap(), &pk);
    }

    /// Transfer transaction should store the sender's public key in their account.
    #[test]
    fn transfer_stores_public_key_in_account() {
        let mut accounts = AccountStore::new();
        let mut validators = ValidatorSet::new();
        let (sk, alice, pk) = test_keypair(7);
        let (_, bob, _) = test_keypair(8);

        accounts.create_account(alice.clone()).unwrap();
        accounts.credit(&alice, opl_to_flake(1000)).unwrap();

        let tx = signed_transfer(&sk, &alice, pk.clone(), &bob, opl_to_flake(10), opl_to_flake(1), 0);
        let result = TransactionDispatcher::apply_transaction(
            &tx, &mut accounts, &mut validators, 1, 100, MAINNET_CHAIN_ID,
        );
        assert!(result.success, "Transfer should succeed: {:?}", result.error);

        let account = accounts.get_account(&alice).unwrap();
        assert!(account.public_key.is_some(), "Public key should be stored after transfer");
        assert_eq!(account.public_key.as_ref().unwrap(), &pk);
    }

    /// Transactions with empty public_key must be rejected — no bypass allowed.
    #[test]
    fn empty_public_key_rejected() {
        let mut accounts = AccountStore::new();
        let mut validators = ValidatorSet::new();
        let alice = hash_to_object_id(b"alice");
        let bob = hash_to_object_id(b"bob");

        accounts.create_account(alice.clone()).unwrap();
        accounts.credit(&alice, opl_to_flake(1000)).unwrap();

        // Unsigned transfer with empty public_key — must be rejected
        let tx = unsigned_transfer(&alice, &bob, opl_to_flake(10), opl_to_flake(1), 0);
        let result = TransactionDispatcher::apply_transaction(
            &tx, &mut accounts, &mut validators, 1, 100, MAINNET_CHAIN_ID,
        );
        assert!(!result.success, "Empty public_key must be rejected");
        // Balance must be unchanged — no funds drained
        assert_eq!(accounts.get_account(&alice).unwrap().balance, opl_to_flake(1000));
    }

    /// Chain ID mismatch must be rejected — prevents cross-chain replay attacks.
    #[test]
    fn wrong_chain_id_rejected() {
        use opolys_core::TESTNET_CHAIN_ID;
        let mut accounts = AccountStore::new();
        let mut validators = ValidatorSet::new();
        let (sk, alice, pk) = test_keypair(1);
        let (_, bob, _) = test_keypair(2);

        accounts.create_account(alice.clone()).unwrap();
        accounts.credit(&alice, opl_to_flake(1000)).unwrap();

        // Create a valid mainnet-signed transaction
        let tx = signed_transfer(&sk, &alice, pk, &bob, opl_to_flake(10), opl_to_flake(1), 0);
        assert_eq!(tx.chain_id, MAINNET_CHAIN_ID);

        // Applying it to a testnet node must fail
        let result = TransactionDispatcher::apply_transaction(
            &tx, &mut accounts, &mut validators, 1, 100, TESTNET_CHAIN_ID,
        );
        assert!(!result.success, "Mainnet tx must be rejected on testnet");
        let err = result.error.unwrap();
        assert!(err.contains("chain_id"), "Error must mention chain_id: {}", err);
    }
}