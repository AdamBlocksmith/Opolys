//! Transaction dispatcher for the Opolys blockchain.
//!
//! This module provides the `TransactionDispatcher` — the single entry point
//! for executing transactions against chain state. Every transaction type
//! (Transfer, RefinerBond, RefinerUnbond) flows through `apply_transaction`,
//! which validates the sender's nonce, dispatches to a type-specific handler,
//! and returns an `ApplyResult`.
//!
//! # Fee model
//!
//! All transaction fees are **burned** (permanently removed from supply).
//! Refiners do not collect fees — they earn from block rewards only.
//! This aligns with Opolys' model as decentralized digital gold: supply
//! expands via emission and contracts via fee burning, with no hard cap.
//!
//! # Per-entry refiner bonds with FIFO unbonding
//!
//! Refiners can hold multiple bond entries, each with its own stake amount
//! and timestamp. `RefinerBond` creates a new entry (or the first
//! one if the refiner doesn't exist yet). `RefinerUnbond { amount }`
//! unbonds the specified amount using FIFO order — oldest entries consumed
//! first. If the amount exceeds an entry's stake, that entry is fully
//! consumed and the remainder comes from the next oldest. Split entries
//! keep their original `bonded_at_timestamp`. Invalid amounts (below
//! MIN_FEE floor or exceeding total stake) result in an error.

use opolys_consensus::account::{AccountStore, TransferResult};
use opolys_consensus::refiner::RefinerSet;
use opolys_core::{
    DECIMAL_PLACES, FLAKES_PER_OPL, FlakeAmount, MIN_FEE, ObjectId, OpolysError,
    SIGNATURE_TYPE_ED25519, Transaction, TransactionAction,
};
use opolys_crypto::{DOMAIN_TX_ID, hash_to_object_id_with_domain, transaction_signing_payload};
use subtle::ConstantTimeEq;

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

#[derive(Clone, Copy)]
struct ExecutionContext {
    block_height: u64,
    block_timestamp: u64,
    total_issued_flakes: FlakeAmount,
}

/// Stateless transaction dispatcher that applies transactions to chain state.
///
/// All methods are associated functions (no instance state) because transaction
/// execution is purely deterministic given its inputs. Every transaction fee is
/// burned (permanently removed from supply) — refiners earn from block rewards,
/// not from fees.
pub struct TransactionDispatcher;

impl TransactionDispatcher {
    fn integer_sqrt_floor(n: u128) -> u128 {
        if n < 2 {
            return n;
        }

        let mut x = n;
        let mut y = x.div_ceil(2);
        while y < x {
            x = y;
            y = (x + n / x) / 2;
        }
        x
    }

    fn dynamic_assay(
        amount: FlakeAmount,
        pressure_numerator: u128,
        pressure_denominator: u128,
        active_refiner_limit: usize,
    ) -> FlakeAmount {
        if amount == 0 || pressure_numerator == 0 || pressure_denominator == 0 {
            return 0;
        }

        let scale = FLAKES_PER_OPL as u128;
        let sqrt_pressure_scaled = Self::integer_sqrt_floor(
            pressure_numerator
                .saturating_mul(scale)
                .saturating_mul(scale)
                / pressure_denominator,
        );
        let sqrt_active_limit_scaled = Self::integer_sqrt_floor(
            (active_refiner_limit as u128)
                .saturating_mul(scale)
                .saturating_mul(scale),
        );
        let denominator = (DECIMAL_PLACES as u128).saturating_mul(sqrt_active_limit_scaled);
        if denominator == 0 {
            return 0;
        }

        ((amount as u128).saturating_mul(sqrt_pressure_scaled) / denominator)
            .min(FlakeAmount::MAX as u128) as FlakeAmount
    }

    fn bond_assay(
        amount: FlakeAmount,
        refiners: &RefinerSet,
        total_issued_flakes: FlakeAmount,
    ) -> FlakeAmount {
        let active_limit = RefinerSet::active_refiner_limit(total_issued_flakes);
        let baseline =
            RefinerSet::minimum_bond_stake(total_issued_flakes) as u128 * active_limit as u128;
        let bonded_after = refiners.total_bonded_stake().saturating_add(amount) as u128;
        Self::dynamic_assay(amount, bonded_after, baseline, active_limit)
    }

    fn unbond_assay(
        amount: FlakeAmount,
        refiners: &RefinerSet,
        total_issued_flakes: FlakeAmount,
    ) -> FlakeAmount {
        let total_bonded = refiners.total_bonded_stake();
        if total_bonded == 0 {
            return 0;
        }

        let active_limit = RefinerSet::active_refiner_limit(total_issued_flakes);
        let baseline =
            RefinerSet::minimum_bond_stake(total_issued_flakes) as u128 * active_limit as u128;
        Self::dynamic_assay(amount, baseline, total_bonded as u128, active_limit)
    }

    fn checked_add_amounts(
        context: &str,
        a: FlakeAmount,
        b: FlakeAmount,
    ) -> Result<FlakeAmount, String> {
        a.checked_add(b)
            .ok_or_else(|| format!("{} overflow: {} + {}", context, a, b))
    }

    fn checked_total_cost(
        context: &str,
        amount: FlakeAmount,
        fee: FlakeAmount,
        assay: FlakeAmount,
    ) -> Result<FlakeAmount, String> {
        let amount_plus_fee = Self::checked_add_amounts(context, amount, fee)?;
        Self::checked_add_amounts(context, amount_plus_fee, assay)
    }

    /// Apply a transaction against the current account and refiner state.
    ///
    /// This is the single entry point for all transaction execution in Opolys.
    /// It first validates the sender's nonce and minimum fee, then dispatches
    /// to the appropriate handler based on `tx.action`:
    /// - `Transfer` → `apply_transfer`
    /// - `RefinerBond` → `apply_bond`
    /// - `RefinerUnbond { amount }` → `apply_unbond`
    ///
    /// `expected_chain_id` must match the network's chain ID (MAINNET_CHAIN_ID=1).
    /// Transactions with a mismatched chain_id are rejected to prevent replay attacks.
    ///
    /// Returns an `ApplyResult` indicating success or failure, along with the
    /// fee amount that was burned.
    pub fn apply_transaction(
        tx: &Transaction,
        accounts: &mut AccountStore,
        refiners: &mut RefinerSet,
        block_height: u64,
        block_timestamp: u64,
        total_issued_flakes: FlakeAmount,
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
        let context = ExecutionContext {
            block_height,
            block_timestamp,
            total_issued_flakes,
        };

        // Verify the sender exists and nonce matches
        if let Some(account) = accounts.get_account(sender) {
            if account.nonce != tx.nonce {
                return ApplyResult::err(&format!(
                    "Invalid nonce: expected {}, got {}",
                    account.nonce, tx.nonce
                ));
            }
            if account.nonce.checked_add(1).is_none() {
                return ApplyResult::err("Nonce overflow");
            }
        } else {
            return ApplyResult::err(&format!("Sender account not found: {}", sender.to_hex()));
        }

        let result = match &tx.action {
            TransactionAction::Transfer { recipient, amount } => {
                Self::apply_transfer(tx, sender, recipient, *amount, accounts)
            }
            TransactionAction::RefinerBond { amount } => {
                Self::apply_bond(tx, sender, *amount, accounts, refiners, context)
            }
            TransactionAction::RefinerUnbond { amount } => {
                Self::apply_unbond(tx, sender, *amount, accounts, refiners, context)
            }
        };

        // After a successful transaction, store the sender's public key in
        // their account. The first real transaction from a pre-funded account
        // registers the key; subsequent transactions update it. This enables
        // future signature verification without a separate key registry.
        if result.success
            && !tx.public_key.is_empty()
            && let Some(account) = accounts.get_account_mut(sender)
        {
            account.public_key = Some(tx.public_key.clone());
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

    /// Bond OPL as refiner stake.
    ///
    /// If the sender is already a refiner, this creates a new bond entry
    /// (top-up) with its own timestamp, or merges
    /// with an existing entry at the same timestamp. Each new entry must be
    /// at least the dynamic minimum from issued supply.
    ///
    /// If the sender is not yet a refiner, this creates a new refiner with
    /// this as their first bond entry (status: Bonding).
    ///
    /// The sender's balance is debited by `stake + fee`, where `stake` becomes
    /// locked refiner stake and `fee` is burned. If the bond fails (e.g.
    /// insufficient balance), the debit is refunded.
    fn apply_bond(
        tx: &Transaction,
        sender: &ObjectId,
        stake: FlakeAmount,
        accounts: &mut AccountStore,
        refiners: &mut RefinerSet,
        context: ExecutionContext,
    ) -> ApplyResult {
        // Minimum stake per new entry is derived from issued supply.
        let minimum_bond = RefinerSet::minimum_bond_stake(context.total_issued_flakes);
        if stake < minimum_bond {
            return ApplyResult::err(&format!(
                "Insufficient bond stake per entry: need {}, got {}",
                minimum_bond, stake
            ));
        }

        // Verify total outflow (stake + fee + assay) doesn't exceed balance.
        // Bond assay rises when the vault is crowded and falls when bonded
        // security is thin, mirroring the cost to assay and store incoming bars.
        let bond_assay = Self::bond_assay(stake, refiners, context.total_issued_flakes);
        let total_needed = match Self::checked_total_cost("Bond cost", stake, tx.fee, bond_assay) {
            Ok(total) => total,
            Err(e) => return ApplyResult::err(&e),
        };
        if let Some(account) = accounts.get_account(sender)
            && account.balance < total_needed
        {
            return ApplyResult::err(&format!(
                "Insufficient balance for bond: need {}, have {}",
                total_needed, account.balance
            ));
        }

        // Debit the sender's account for stake + fee
        if let Err(e) = accounts.debit(sender, total_needed) {
            return ApplyResult::err(&e.to_string());
        }

        // Register the bond (creates new entry or merges with same-timestamp entry)
        let sender_clone = sender.clone();
        if let Err(e) = refiners.bond(
            sender_clone,
            stake,
            context.block_height,
            context.block_timestamp,
            context.total_issued_flakes,
        ) {
            if let Err(refund_error) = accounts.credit(sender, total_needed) {
                return ApplyResult::err(&format!(
                    "Bond failed ({}) and refund failed: {}",
                    e, refund_error
                ));
            }
            return ApplyResult::err(&e);
        }

        // Increment the sender's nonce to prevent replay
        if let Some(account) = accounts.get_account_mut(sender) {
            account.nonce += 1;
        }

        // Fee + bond assay are both burned (debited from sender, not credited to anyone)
        // Bond assay mirrors gold: assay fee to enter the vault
        match Self::checked_add_amounts("Bond burned amount", tx.fee, bond_assay) {
            Ok(burned) => ApplyResult::ok(burned),
            Err(e) => ApplyResult::err(&e),
        }
    }

    /// Unbond `amount` Flakes from the refiner using FIFO order.
    ///
    /// The oldest entries are consumed first. If the amount exceeds an entry's
    /// stake, that entry is fully consumed and the remainder comes from the
    /// next oldest. The unbonded stake enters the unbonding queue and is
    /// returned after `UNBONDING_DELAY_BLOCKS` (960 blocks = one epoch).
    ///
    /// The transaction fee is burned from the sender's balance immediately.
    /// If the refiner has insufficient stake, the transaction fails with
    /// no fee burn and no nonce advance.
    ///
    fn apply_unbond(
        tx: &Transaction,
        sender: &ObjectId,
        amount: FlakeAmount,
        accounts: &mut AccountStore,
        refiners: &mut RefinerSet,
        context: ExecutionContext,
    ) -> ApplyResult {
        // Check that the refiner exists
        if refiners.get_refiner(sender).is_none() {
            return ApplyResult::err("Refiner not bonded");
        }
        if amount == 0 {
            return ApplyResult::err("Unbond amount must be greater than zero");
        }

        // Pre-check fees before unbonding. Both the transaction fee and the
        // dynamic unbond assay must be payable. The assay rises when bonded
        // security is thin and falls when the vault is well-stocked.
        let max_assay = Self::unbond_assay(amount, refiners, context.total_issued_flakes);
        let total_fee = match Self::checked_add_amounts("Unbond fee", tx.fee, max_assay) {
            Ok(total) => total,
            Err(e) => return ApplyResult::err(&e),
        };
        if let Some(account) = accounts.get_account(sender)
            && account.balance < total_fee
        {
            return ApplyResult::err(&format!(
                "Insufficient balance for unbond fees: need {}, have {}",
                total_fee, account.balance
            ));
        }

        // Perform FIFO unbond — oldest entries consumed first, queued for delayed return
        let unbonded = match refiners.unbond_amount(sender, amount, context.block_height) {
            Ok(stake) => stake,
            Err(e) => return ApplyResult::err(&e),
        };

        if unbonded == 0 {
            return ApplyResult::err("No stake unbonded");
        }

        // The unbonded stake enters the unbonding queue and will be returned
        // after UNBONDING_DELAY_BLOCKS. It is NOT credited to the sender's
        // account immediately.

        // The current unbond path rejects requests above total stake, so the
        // prechecked dynamic assay is exactly the assay owed for this withdrawal.
        let unbond_assay = max_assay;
        let total_fee = match Self::checked_add_amounts("Unbond fee", tx.fee, unbond_assay) {
            Ok(total) => total,
            Err(e) => return ApplyResult::err(&e),
        };

        // Deduct fees from sender's balance (already pre-checked above)
        if let Some(account) = accounts.get_account_mut(sender) {
            account.balance -= total_fee;
        }

        // Increment the sender's nonce
        if let Some(account) = accounts.get_account_mut(sender) {
            account.nonce += 1;
        }

        // Both tx fee and unbond assay are burned
        ApplyResult::ok(total_fee)
    }
}

/// Basic pre-validation of a transaction before it enters the mempool.
///
/// Checks nonce correctness, minimum fee, and balance sufficiency for the
/// transaction's non-assay cost. Dynamic bond/unbond assays depend on live
/// refiner state and are enforced during block execution.
///
/// This is a fast check to reject obviously invalid transactions before
/// they consume mempool space or network bandwidth.
pub fn validate_transaction_basic(
    tx: &Transaction,
    sender_balance: FlakeAmount,
    sender_nonce: u64,
) -> Result<(), OpolysError> {
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

    // Total cost depends on the transaction type. Dynamic refiner assays depend
    // on live refiner state, so this basic helper only checks non-assay cost.
    let checked_add = |context: &str, a: FlakeAmount, b: FlakeAmount| {
        a.checked_add(b).ok_or_else(|| {
            OpolysError::InvalidParams(format!("{} overflow: {} + {}", context, a, b))
        })
    };

    let total_cost = match &tx.action {
        TransactionAction::Transfer { amount, .. } => {
            checked_add("Transfer cost", *amount, tx.fee)?
        }
        TransactionAction::RefinerBond { amount } => checked_add("Bond cost", *amount, tx.fee)?,
        TransactionAction::RefinerUnbond { .. } => tx.fee,
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
    let expected_tx_id = compute_tx_id(&tx.sender, &tx.action, tx.fee, tx.nonce, tx.chain_id)?;
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
        Err(_) => {
            return Err(OpolysError::InvalidTransaction(
                "Public key conversion failed".to_string(),
            ));
        }
    };
    let derived_object_id = opolys_crypto::ed25519_public_key_to_object_id(&pk_bytes);
    if !bool::from(tx.sender.as_bytes().ct_eq(derived_object_id.as_bytes())) {
        return Err(OpolysError::InvalidTransaction(format!(
            "Public key does not match sender: Blake3(pk)={}, sender={}",
            derived_object_id.to_hex(),
            tx.sender.to_hex()
        )));
    }

    // 6. Verify ed25519 signature over domain-separated transaction payload
    let unsigned_data =
        transaction_signing_payload(&tx.sender, &tx.action, tx.fee, tx.nonce, tx.chain_id)?;

    if !opolys_crypto::verify_ed25519(&tx.public_key, &unsigned_data, &tx.signature) {
        return Err(OpolysError::InvalidSignature);
    }

    Ok(())
}

/// Compute the expected transaction ID from the transaction fields.
///
/// Matches the wallet's `TransactionSigner::compute_tx_id` function exactly:
/// Blake3-256(domain || borsh(sender, action, fee, nonce, chain_id)).
fn compute_tx_id(
    sender: &ObjectId,
    action: &TransactionAction,
    fee: FlakeAmount,
    nonce: u64,
    chain_id: u64,
) -> Result<ObjectId, OpolysError> {
    let data = borsh::to_vec(&(sender.clone(), action, fee, nonce, chain_id)).map_err(|e| {
        OpolysError::SerializationError(format!("Transaction ID serialization failed: {}", e))
    })?;
    Ok(hash_to_object_id_with_domain(DOMAIN_TX_ID, &data))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use opolys_core::{EPOCH, MAINNET_CHAIN_ID, MIN_BOND_STAKE, opl_to_flake};
    use opolys_crypto::hash_to_object_id;

    /// Deterministic test keypair from a single seed byte.
    fn test_keypair(seed: u8) -> (SigningKey, ObjectId, Vec<u8>) {
        let signing_key = SigningKey::from_bytes(&[seed; 32]);
        let pk = signing_key.verifying_key();
        let pk_bytes = pk.as_bytes().to_vec();
        let id = opolys_crypto::ed25519_public_key_to_object_id(pk.as_bytes());
        (signing_key, id, pk_bytes)
    }

    fn signed_transfer(
        sk: &SigningKey,
        sender: &ObjectId,
        pk: Vec<u8>,
        to: &ObjectId,
        amount: FlakeAmount,
        fee: FlakeAmount,
        nonce: u64,
    ) -> Transaction {
        let action = TransactionAction::Transfer {
            recipient: to.clone(),
            amount,
        };
        let tx_id = compute_tx_id(sender, &action, fee, nonce, MAINNET_CHAIN_ID).unwrap();
        let msg =
            transaction_signing_payload(sender, &action, fee, nonce, MAINNET_CHAIN_ID).unwrap();
        Transaction {
            tx_id,
            sender: sender.clone(),
            action,
            fee,
            signature: sk.sign(&msg).to_bytes().to_vec(),
            signature_type: SIGNATURE_TYPE_ED25519,
            nonce,
            chain_id: MAINNET_CHAIN_ID,
            data: vec![],
            public_key: pk,
        }
    }

    fn signed_bond(
        sk: &SigningKey,
        sender: &ObjectId,
        pk: Vec<u8>,
        amount: FlakeAmount,
        fee: FlakeAmount,
        nonce: u64,
    ) -> Transaction {
        let action = TransactionAction::RefinerBond { amount };
        let tx_id = compute_tx_id(sender, &action, fee, nonce, MAINNET_CHAIN_ID).unwrap();
        let msg =
            transaction_signing_payload(sender, &action, fee, nonce, MAINNET_CHAIN_ID).unwrap();
        Transaction {
            tx_id,
            sender: sender.clone(),
            action,
            fee,
            signature: sk.sign(&msg).to_bytes().to_vec(),
            signature_type: SIGNATURE_TYPE_ED25519,
            nonce,
            chain_id: MAINNET_CHAIN_ID,
            data: vec![],
            public_key: pk,
        }
    }

    fn signed_unbond(
        sk: &SigningKey,
        sender: &ObjectId,
        pk: Vec<u8>,
        amount: FlakeAmount,
        fee: FlakeAmount,
        nonce: u64,
    ) -> Transaction {
        let action = TransactionAction::RefinerUnbond { amount };
        let tx_id = compute_tx_id(sender, &action, fee, nonce, MAINNET_CHAIN_ID).unwrap();
        let msg =
            transaction_signing_payload(sender, &action, fee, nonce, MAINNET_CHAIN_ID).unwrap();
        Transaction {
            tx_id,
            sender: sender.clone(),
            action,
            fee,
            signature: sk.sign(&msg).to_bytes().to_vec(),
            signature_type: SIGNATURE_TYPE_ED25519,
            nonce,
            chain_id: MAINNET_CHAIN_ID,
            data: vec![],
            public_key: pk,
        }
    }

    /// Unsigned helper used only for validate_transaction_basic tests (no sig check).
    fn unsigned_transfer(
        sender: &ObjectId,
        recipient: &ObjectId,
        amount: FlakeAmount,
        fee: FlakeAmount,
        nonce: u64,
    ) -> Transaction {
        let action = TransactionAction::Transfer {
            recipient: recipient.clone(),
            amount,
        };
        let tx_id = compute_tx_id(sender, &action, fee, nonce, MAINNET_CHAIN_ID).unwrap();
        Transaction {
            tx_id,
            sender: sender.clone(),
            action,
            fee,
            signature: vec![],
            signature_type: SIGNATURE_TYPE_ED25519,
            nonce,
            chain_id: MAINNET_CHAIN_ID,
            data: vec![],
            public_key: vec![],
        }
    }

    /// Unsigned helper used only for validate_transaction_basic tests (no sig check).
    fn unsigned_bond(
        sender: &ObjectId,
        amount: FlakeAmount,
        fee: FlakeAmount,
        nonce: u64,
    ) -> Transaction {
        let action = TransactionAction::RefinerBond { amount };
        let tx_id = compute_tx_id(sender, &action, fee, nonce, MAINNET_CHAIN_ID).unwrap();
        Transaction {
            tx_id,
            sender: sender.clone(),
            action,
            fee,
            signature: vec![],
            signature_type: SIGNATURE_TYPE_ED25519,
            nonce,
            chain_id: MAINNET_CHAIN_ID,
            data: vec![],
            public_key: vec![],
        }
    }

    #[test]
    fn transfer_success() {
        let mut accounts = AccountStore::new();
        let mut refiners = RefinerSet::new();
        let (sk, alice, pk) = test_keypair(1);
        let (_, bob, _) = test_keypair(2);

        accounts.create_account(alice.clone()).unwrap();
        accounts.credit(&alice, opl_to_flake(1000)).unwrap();

        let tx = signed_transfer(&sk, &alice, pk, &bob, opl_to_flake(10), opl_to_flake(1), 0);
        let result = TransactionDispatcher::apply_transaction(
            &tx,
            &mut accounts,
            &mut refiners,
            1,
            100,
            0,
            MAINNET_CHAIN_ID,
        );
        assert!(
            result.success,
            "Transfer should succeed: {:?}",
            result.error
        );
        assert_eq!(result.fee_burned, opl_to_flake(1));
        assert_eq!(
            accounts.get_account(&alice).unwrap().balance,
            opl_to_flake(989)
        );
        assert_eq!(
            accounts.get_account(&bob).unwrap().balance,
            opl_to_flake(10)
        );
    }

    #[test]
    fn transfer_insufficient_balance() {
        let mut accounts = AccountStore::new();
        let mut refiners = RefinerSet::new();
        let (sk, alice, pk) = test_keypair(1);
        let (_, bob, _) = test_keypair(2);

        accounts.create_account(alice.clone()).unwrap();
        accounts.credit(&alice, 100).unwrap();

        let tx = signed_transfer(&sk, &alice, pk, &bob, 200, 10, 0);
        let result = TransactionDispatcher::apply_transaction(
            &tx,
            &mut accounts,
            &mut refiners,
            1,
            100,
            0,
            MAINNET_CHAIN_ID,
        );
        assert!(!result.success);
    }

    #[test]
    fn transfer_nonce_overflow_rejected_before_mutation() {
        let mut accounts = AccountStore::new();
        let mut refiners = RefinerSet::new();
        let (sk, alice, pk) = test_keypair(1);
        let (_, bob, _) = test_keypair(2);

        accounts.create_account(alice.clone()).unwrap();
        accounts.credit(&alice, opl_to_flake(100)).unwrap();
        accounts.get_account_mut(&alice).unwrap().nonce = u64::MAX;

        let tx = signed_transfer(&sk, &alice, pk, &bob, opl_to_flake(1), MIN_FEE, u64::MAX);
        let result = TransactionDispatcher::apply_transaction(
            &tx,
            &mut accounts,
            &mut refiners,
            1,
            100,
            0,
            MAINNET_CHAIN_ID,
        );

        assert!(!result.success);
        assert_eq!(result.error.as_deref(), Some("Nonce overflow"));
        assert_eq!(
            accounts.get_account(&alice).unwrap().balance,
            opl_to_flake(100)
        );
        assert!(accounts.get_account(&bob).is_none());
    }

    /// Bond refiner with sufficient stake — should succeed.
    /// Alice bonds 1 OPL (MIN_BOND_STAKE) with a 1 OPL fee.
    #[test]
    fn bond_refiner_success() {
        let mut accounts = AccountStore::new();
        let mut refiners = RefinerSet::new();
        let (sk, alice, pk) = test_keypair(1);

        accounts.create_account(alice.clone()).unwrap();
        accounts.credit(&alice, opl_to_flake(200)).unwrap();

        let bond_assay = TransactionDispatcher::bond_assay(MIN_BOND_STAKE, &refiners, 0);
        let tx = signed_bond(&sk, &alice, pk, MIN_BOND_STAKE, opl_to_flake(1), 0);
        let result = TransactionDispatcher::apply_transaction(
            &tx,
            &mut accounts,
            &mut refiners,
            1,
            100,
            0,
            MAINNET_CHAIN_ID,
        );
        assert!(result.success, "Bond should succeed: {:?}", result.error);
        assert_eq!(refiners.refiner_count(), 1);
        assert_eq!(
            accounts.get_account(&alice).unwrap().balance,
            opl_to_flake(200) - MIN_BOND_STAKE - opl_to_flake(1) - bond_assay
        );
    }

    #[test]
    fn dynamic_assay_moves_with_vault_pressure() {
        let amount = opl_to_flake(100);
        let active_limit = RefinerSet::active_refiner_limit(1_000_000 * opl_to_flake(1));

        let baseline = TransactionDispatcher::dynamic_assay(amount, 1, 1, active_limit);
        let low_pressure = TransactionDispatcher::dynamic_assay(amount, 1, 4, active_limit);
        let high_pressure = TransactionDispatcher::dynamic_assay(amount, 4, 1, active_limit);

        assert!(low_pressure < baseline);
        assert!(high_pressure > baseline);
        assert!(high_pressure >= baseline.saturating_mul(2));
    }

    /// Bond refiner with insufficient stake — should fail.
    #[test]
    fn bond_refiner_below_minimum() {
        let mut accounts = AccountStore::new();
        let mut refiners = RefinerSet::new();
        let (sk, alice, pk) = test_keypair(1);

        accounts.create_account(alice.clone()).unwrap();
        accounts.credit(&alice, opl_to_flake(200)).unwrap();

        let too_low = 50; // Below MIN_BOND_STAKE (1 OPL = 1,000,000 flakes)
        let tx = signed_bond(&sk, &alice, pk, too_low, opl_to_flake(1), 0);
        let result = TransactionDispatcher::apply_transaction(
            &tx,
            &mut accounts,
            &mut refiners,
            1,
            100,
            0,
            MAINNET_CHAIN_ID,
        );
        assert!(!result.success);
    }

    /// Dynamic minimum bond grows with issued supply.
    #[test]
    fn bond_refiner_respects_dynamic_minimum() {
        let mut accounts = AccountStore::new();
        let mut refiners = RefinerSet::new();
        let (sk, alice, pk) = test_keypair(1);

        accounts.create_account(alice.clone()).unwrap();
        accounts.credit(&alice, opl_to_flake(10_000)).unwrap();

        let issued = 25_000_000 * opl_to_flake(1);
        let required = RefinerSet::minimum_bond_stake(issued);
        assert_eq!(required, opl_to_flake(5_000));

        let tx = signed_bond(&sk, &alice, pk, required - 1, opl_to_flake(1), 0);
        let result = TransactionDispatcher::apply_transaction(
            &tx,
            &mut accounts,
            &mut refiners,
            1,
            100,
            issued,
            MAINNET_CHAIN_ID,
        );
        assert!(!result.success);
        assert!(
            result
                .error
                .unwrap()
                .contains("Insufficient bond stake per entry")
        );
    }

    /// Top-up: existing refiner bonds again, creating a second entry.
    #[test]
    fn bond_top_up_creates_new_entry() {
        let mut accounts = AccountStore::new();
        let mut refiners = RefinerSet::new();
        let (sk, alice, pk) = test_keypair(1);

        accounts.create_account(alice.clone()).unwrap();
        accounts.credit(&alice, opl_to_flake(500)).unwrap();

        // First bond: 1 OPL
        let tx1 = signed_bond(&sk, &alice, pk.clone(), MIN_BOND_STAKE, opl_to_flake(1), 0);
        let result1 = TransactionDispatcher::apply_transaction(
            &tx1,
            &mut accounts,
            &mut refiners,
            1,
            100,
            0,
            MAINNET_CHAIN_ID,
        );
        assert!(result1.success);

        // Second bond (top-up): 2 OPL
        let tx2 = signed_bond(&sk, &alice, pk, MIN_BOND_STAKE * 2, opl_to_flake(1), 1);
        let result2 = TransactionDispatcher::apply_transaction(
            &tx2,
            &mut accounts,
            &mut refiners,
            2,
            200,
            0,
            MAINNET_CHAIN_ID,
        );
        assert!(
            result2.success,
            "Top-up bond should succeed: {:?}",
            result2.error
        );

        let v = refiners.get_refiner(&alice).unwrap();
        assert_eq!(v.entries.len(), 2);
        assert_eq!(v.total_stake(), MIN_BOND_STAKE * 3);
        assert_eq!(refiners.refiner_count(), 1);
    }

    /// Unbond using FIFO — oldest entries consumed first.
    #[test]
    fn unbond_fifo_partial() {
        let mut accounts = AccountStore::new();
        let mut refiners = RefinerSet::new();
        let (sk, alice, pk) = test_keypair(1);

        accounts.create_account(alice.clone()).unwrap();
        accounts.credit(&alice, opl_to_flake(500)).unwrap();

        // Bond 1 OPL at t=100
        let bond_assay_1 = TransactionDispatcher::bond_assay(MIN_BOND_STAKE, &refiners, 0);
        let tx1 = signed_bond(&sk, &alice, pk.clone(), MIN_BOND_STAKE, opl_to_flake(1), 0);
        let r1 = TransactionDispatcher::apply_transaction(
            &tx1,
            &mut accounts,
            &mut refiners,
            1,
            100,
            0,
            MAINNET_CHAIN_ID,
        );
        assert!(r1.success, "First bond should succeed: {:?}", r1.error);

        // Bond 2 OPL at t=1000
        let bond_assay_2 = TransactionDispatcher::bond_assay(MIN_BOND_STAKE * 2, &refiners, 0);
        let tx2 = signed_bond(
            &sk,
            &alice,
            pk.clone(),
            MIN_BOND_STAKE * 2,
            opl_to_flake(1),
            1,
        );
        let r2 = TransactionDispatcher::apply_transaction(
            &tx2,
            &mut accounts,
            &mut refiners,
            2,
            1000,
            0,
            MAINNET_CHAIN_ID,
        );
        assert!(
            r2.success,
            "Second bond (top-up) should succeed: {:?}",
            r2.error
        );

        // Alice balance: 500 - 1 (stake) - 1 (fee) - bond_assay_1 - 2 (stake) - 1 (fee) - bond_assay_2
        assert_eq!(
            accounts.get_account(&alice).unwrap().balance,
            opl_to_flake(500)
                - MIN_BOND_STAKE
                - opl_to_flake(1)
                - bond_assay_1
                - MIN_BOND_STAKE * 2
                - opl_to_flake(1)
                - bond_assay_2
        );

        // Unbond 1.5 OPL — consumes first entry (1 OPL) + 0.5 OPL from second entry
        let unbond_amount = MIN_BOND_STAKE + MIN_BOND_STAKE / 2; // 1.5 OPL
        let unbond_assay = TransactionDispatcher::unbond_assay(unbond_amount, &refiners, 0);
        let tx3 = signed_unbond(
            &sk,
            &alice,
            pk,
            MIN_BOND_STAKE + MIN_BOND_STAKE / 2,
            opl_to_flake(1),
            2,
        );
        let result = TransactionDispatcher::apply_transaction(
            &tx3,
            &mut accounts,
            &mut refiners,
            3,
            300,
            0,
            MAINNET_CHAIN_ID,
        );
        assert!(result.success, "Unbond should succeed: {:?}", result.error);

        let v = refiners.get_refiner(&alice).unwrap();
        assert_eq!(v.entries.len(), 1);
        // Remaining: 2 - 0.5 = 1.5 OPL
        assert_eq!(v.total_stake(), MIN_BOND_STAKE * 3 / 2);

        // Unbonded stake goes into the unbonding queue, not immediately returned
        assert_eq!(refiners.unbonding_queue.len(), 1);
        assert_eq!(
            refiners.unbonding_queue[0].amount,
            MIN_BOND_STAKE + MIN_BOND_STAKE / 2
        );
        assert_eq!(refiners.unbonding_queue[0].matures_at, 3 + EPOCH);

        // Alice's balance after unbond: previous balance - 1 (fee) - unbond_assay
        let prev_balance = opl_to_flake(500)
            - MIN_BOND_STAKE
            - opl_to_flake(1)
            - bond_assay_1
            - MIN_BOND_STAKE * 2
            - opl_to_flake(1)
            - bond_assay_2;
        assert_eq!(
            accounts.get_account(&alice).unwrap().balance,
            prev_balance - opl_to_flake(1) - unbond_assay
        );
    }

    /// Unbond more than total stake — unbonds all available.
    #[test]
    fn unbond_more_than_stake() {
        let mut accounts = AccountStore::new();
        let mut refiners = RefinerSet::new();
        let (sk, alice, pk) = test_keypair(1);

        accounts.create_account(alice.clone()).unwrap();
        accounts.credit(&alice, opl_to_flake(200)).unwrap();

        let tx1 = signed_bond(&sk, &alice, pk.clone(), MIN_BOND_STAKE, opl_to_flake(1), 0);
        let r = TransactionDispatcher::apply_transaction(
            &tx1,
            &mut accounts,
            &mut refiners,
            1,
            100,
            0,
            MAINNET_CHAIN_ID,
        );
        assert!(r.success, "Bond should succeed: {:?}", r.error);

        // Try to unbond 10x the total stake
        let tx2 = signed_unbond(&sk, &alice, pk, MIN_BOND_STAKE * 10, opl_to_flake(1), 1);
        let result = TransactionDispatcher::apply_transaction(
            &tx2,
            &mut accounts,
            &mut refiners,
            2,
            200,
            0,
            MAINNET_CHAIN_ID,
        );
        assert!(!result.success);
        assert!(
            result
                .error
                .unwrap()
                .contains("Cannot unbond more than total stake")
        );
        assert_eq!(refiners.refiner_count(), 1);
        assert!(refiners.unbonding_queue.is_empty());
    }

    /// Unbond a non-existent refiner — should fail.
    #[test]
    fn unbond_nonexistent_refiner() {
        let mut accounts = AccountStore::new();
        let mut refiners = RefinerSet::new();
        let (sk, alice, pk) = test_keypair(1);

        // Alice has no account and is not a refiner
        let tx = signed_unbond(&sk, &alice, pk, MIN_BOND_STAKE, opl_to_flake(1), 0);
        let result = TransactionDispatcher::apply_transaction(
            &tx,
            &mut accounts,
            &mut refiners,
            1,
            100,
            0,
            MAINNET_CHAIN_ID,
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
        let total_cost = MIN_BOND_STAKE + opl_to_flake(1);
        assert!(validate_transaction_basic(&tx, total_cost, 0).is_ok());
        assert!(validate_transaction_basic(&tx, total_cost - 1, 0).is_err());
    }

    /// Bond transaction should store the sender's public key in their account.
    #[test]
    fn bond_stores_public_key_in_account() {
        let mut accounts = AccountStore::new();
        let mut refiners = RefinerSet::new();
        let (sk, alice, pk) = test_keypair(42);

        accounts.create_account(alice.clone()).unwrap();
        accounts.credit(&alice, opl_to_flake(200)).unwrap();

        let tx = signed_bond(&sk, &alice, pk.clone(), MIN_BOND_STAKE, opl_to_flake(1), 0);
        let result = TransactionDispatcher::apply_transaction(
            &tx,
            &mut accounts,
            &mut refiners,
            1,
            100,
            0,
            MAINNET_CHAIN_ID,
        );
        assert!(result.success, "Bond should succeed: {:?}", result.error);

        let account = accounts.get_account(&alice).unwrap();
        assert!(
            account.public_key.is_some(),
            "Public key should be stored after bond"
        );
        assert_eq!(account.public_key.as_ref().unwrap(), &pk);
    }

    /// Transfer transaction should store the sender's public key in their account.
    #[test]
    fn transfer_stores_public_key_in_account() {
        let mut accounts = AccountStore::new();
        let mut refiners = RefinerSet::new();
        let (sk, alice, pk) = test_keypair(7);
        let (_, bob, _) = test_keypair(8);

        accounts.create_account(alice.clone()).unwrap();
        accounts.credit(&alice, opl_to_flake(1000)).unwrap();

        let tx = signed_transfer(
            &sk,
            &alice,
            pk.clone(),
            &bob,
            opl_to_flake(10),
            opl_to_flake(1),
            0,
        );
        let result = TransactionDispatcher::apply_transaction(
            &tx,
            &mut accounts,
            &mut refiners,
            1,
            100,
            0,
            MAINNET_CHAIN_ID,
        );
        assert!(
            result.success,
            "Transfer should succeed: {:?}",
            result.error
        );

        let account = accounts.get_account(&alice).unwrap();
        assert!(
            account.public_key.is_some(),
            "Public key should be stored after transfer"
        );
        assert_eq!(account.public_key.as_ref().unwrap(), &pk);
    }

    /// Transactions with empty public_key must be rejected — no bypass allowed.
    #[test]
    fn empty_public_key_rejected() {
        let mut accounts = AccountStore::new();
        let mut refiners = RefinerSet::new();
        let alice = hash_to_object_id(b"alice");
        let bob = hash_to_object_id(b"bob");

        accounts.create_account(alice.clone()).unwrap();
        accounts.credit(&alice, opl_to_flake(1000)).unwrap();

        // Unsigned transfer with empty public_key — must be rejected
        let tx = unsigned_transfer(&alice, &bob, opl_to_flake(10), opl_to_flake(1), 0);
        let result = TransactionDispatcher::apply_transaction(
            &tx,
            &mut accounts,
            &mut refiners,
            1,
            100,
            0,
            MAINNET_CHAIN_ID,
        );
        assert!(!result.success, "Empty public_key must be rejected");
        // Balance must be unchanged — no funds drained
        assert_eq!(
            accounts.get_account(&alice).unwrap().balance,
            opl_to_flake(1000)
        );
    }

    /// Chain ID mismatch must be rejected — prevents cross-chain replay attacks.
    #[test]
    fn wrong_chain_id_rejected() {
        let wrong_chain_id: u64 = 2;
        let mut accounts = AccountStore::new();
        let mut refiners = RefinerSet::new();
        let (sk, alice, pk) = test_keypair(1);
        let (_, bob, _) = test_keypair(2);

        accounts.create_account(alice.clone()).unwrap();
        accounts.credit(&alice, opl_to_flake(1000)).unwrap();

        // Create a valid mainnet-signed transaction
        let tx = signed_transfer(&sk, &alice, pk, &bob, opl_to_flake(10), opl_to_flake(1), 0);
        assert_eq!(tx.chain_id, MAINNET_CHAIN_ID);

        // Applying it with a different chain_id must fail
        let result = TransactionDispatcher::apply_transaction(
            &tx,
            &mut accounts,
            &mut refiners,
            1,
            100,
            0,
            wrong_chain_id,
        );
        assert!(
            !result.success,
            "Mainnet tx must be rejected with wrong chain_id"
        );
        let err = result.error.unwrap();
        assert!(
            err.contains("chain_id"),
            "Error must mention chain_id: {}",
            err
        );
    }
}
