//! # Account management for the Opolys ledger.
//!
//! Every participant in the Opolys network is represented by an [`Account`]
//! identified by its Blake3 `ObjectId`. Accounts carry a balance (denominated
//! in flakes, the smallest unit of $OPL) and a monotonically-increasing nonce
//! for replay protection.
//!
//! The [`AccountStore`] provides in-memory CRUD operations including credits,
//! debits, and atomic transfers. Transfer fees are **burned** — they leave the
//! circulating supply entirely, aligning with Opolys' market-driven fee model.

use borsh::{BorshDeserialize, BorshSerialize};
use opolys_core::{FlakeAmount, ObjectId, OpolysError};
use opolys_crypto::{Blake3Hasher, DOMAIN_STATE_ROOT};

/// A single account in the Opolys ledger.
///
/// Every account has a balance (in flakes), a nonce (for replay protection),
/// an optional ed25519 public key (for signature verification), and is
/// identified by its ObjectId (Blake3 hash of the public key).
///
/// The public key is `None` for genesis/pre-funded accounts and `Some` for
/// accounts created by transactions (Bond, Transfer). Signature verification
/// requires the public key to derive the expected ObjectId and verify the
/// ed25519 signature.
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct Account {
    /// Blake3-based unique identifier derived from the account's public key.
    pub object_id: ObjectId,
    /// Balance in flakes (1 OPL = 1,000,000 flakes).
    pub balance: FlakeAmount,
    /// Monotonically-increasing nonce used to prevent transaction replay.
    pub nonce: u64,
    /// Ed25519 public key (32 bytes) for signature verification.
    /// Stored alongside the account so that transaction signatures can be
    /// verified without a separate key registry.
    /// If `None`, the account was created by genesis or pre-funding and
    /// cannot send transactions until a public key is registered.
    pub public_key: Option<Vec<u8>>,
}

impl Account {
    /// Create a new account with zero balance, zero nonce, and no public key.
    ///
    /// Accounts created by genesis or pre-funding may not have a public key
    /// registered yet. To register a public key, use `set_public_key()`.
    pub fn new(object_id: ObjectId) -> Self {
        Account {
            object_id,
            balance: 0,
            nonce: 0,
            public_key: None,
        }
    }

    /// Create a new account with a public key.
    ///
    /// The `object_id` must be `Blake3(public_key)` — callers should verify
    /// this invariant before calling. The public key enables signature
    /// verification for transactions from this account.
    pub fn with_public_key(object_id: ObjectId, public_key: Vec<u8>) -> Self {
        Account {
            object_id,
            balance: 0,
            nonce: 0,
            public_key: Some(public_key),
        }
    }

    /// Returns `true` if the account holds at least `amount` flakes.
    pub fn can_spend(&self, amount: FlakeAmount) -> bool {
        self.balance >= amount
    }
}

/// In-memory store for all Opolys accounts, backed by a `HashMap`.
///
/// Supports persistence via [`all_accounts`] and [`load_from_accounts`], and
/// provides atomic transfer semantics where the sender's balance and nonce
/// are updated in a single logical operation.
#[derive(Debug, Clone)]
pub struct AccountStore {
    accounts: std::collections::HashMap<ObjectId, Account>,
}

impl AccountStore {
    /// Create an empty account store.
    pub fn new() -> Self {
        AccountStore {
            accounts: std::collections::HashMap::new(),
        }
    }

    /// Return all accounts as a serializable Vec. Used for persistence.
    pub fn all_accounts(&self) -> Vec<Account> {
        self.accounts.values().cloned().collect()
    }

    /// Load accounts from a serialized Vec. Used for state restoration.
    pub fn load_from_accounts(accounts: Vec<Account>) -> Self {
        let mut store = AccountStore::new();
        for account in accounts {
            store.accounts.insert(account.object_id.clone(), account);
        }
        store
    }

    /// Register a brand-new account. Fails if the ObjectId already exists.
    pub fn create_account(&mut self, object_id: ObjectId) -> Result<&Account, OpolysError> {
        if self.accounts.contains_key(&object_id) {
            return Err(OpolysError::AccountNotFound(format!(
                "Account already exists: {}",
                object_id.to_hex()
            )));
        }
        self.accounts
            .insert(object_id.clone(), Account::new(object_id.clone()));
        Ok(self.accounts.get(&object_id).unwrap())
    }

    /// Look up an account by its ObjectId. Returns `None` if not found.
    pub fn get_account(&self, object_id: &ObjectId) -> Option<&Account> {
        self.accounts.get(object_id)
    }

    /// Look up an account by ObjectId with mutable access. Returns `None` if not found.
    pub fn get_account_mut(&mut self, object_id: &ObjectId) -> Option<&mut Account> {
        self.accounts.get_mut(object_id)
    }

    /// Add `amount` flakes to the account's balance. Fails on overflow so
    /// consensus cannot silently mint by saturating at `u64::MAX`.
    pub fn credit(&mut self, object_id: &ObjectId, amount: FlakeAmount) -> Result<(), OpolysError> {
        let account = self
            .accounts
            .get_mut(object_id)
            .ok_or_else(|| OpolysError::AccountNotFound(object_id.to_hex()))?;
        account.balance = account.balance.checked_add(amount).ok_or_else(|| {
            OpolysError::InvalidParams(format!(
                "Balance overflow when crediting {} flakes to {}",
                amount,
                object_id.to_hex()
            ))
        })?;
        Ok(())
    }

    /// Subtract `amount` flakes from the account's balance. Fails if the
    /// account doesn't exist or holds insufficient funds.
    pub fn debit(&mut self, object_id: &ObjectId, amount: FlakeAmount) -> Result<(), OpolysError> {
        let account = self
            .accounts
            .get_mut(object_id)
            .ok_or_else(|| OpolysError::AccountNotFound(object_id.to_hex()))?;
        if account.balance < amount {
            return Err(OpolysError::InsufficientBalance {
                need: amount,
                have: account.balance,
            });
        }
        account.balance -= amount;
        Ok(())
    }

    /// Atomically transfer `amount` flakes from one account to another,
    /// burning `fee` flakes from the sender (not delivered to the recipient).
    ///
    /// The sender must hold `amount + fee` flakes. On success the sender's
    /// nonce increments by one and a [`TransferResult`] is returned.
    ///
    /// If the recipient does not yet exist, it is auto-created with zero
    /// balance — consistent with Opolys' permissionless account model.
    pub fn transfer(
        &mut self,
        from: &ObjectId,
        to: &ObjectId,
        amount: FlakeAmount,
        fee: FlakeAmount,
    ) -> Result<TransferResult, OpolysError> {
        // The sender must cover both the transfer amount and the burned fee.
        let total_needed = amount.checked_add(fee).ok_or_else(|| {
            OpolysError::InvalidParams(format!(
                "Transfer cost overflow: amount {} + fee {}",
                amount, fee
            ))
        })?;
        let from_account = self
            .accounts
            .get(from)
            .ok_or_else(|| OpolysError::AccountNotFound(from.to_hex()))?;

        if from_account.balance < total_needed {
            return Err(OpolysError::InsufficientBalance {
                need: total_needed,
                have: from_account.balance,
            });
        }

        let from_nonce = from_account.nonce;
        from_nonce
            .checked_add(1)
            .ok_or_else(|| OpolysError::InvalidParams("Nonce overflow".to_string()))?;

        // Auto-create the recipient account if it doesn't exist yet.
        let to_exists = self.accounts.contains_key(to);
        if !to_exists {
            self.accounts.insert(to.clone(), Account::new(to.clone()));
        }

        if from != to {
            let recipient_balance = self.accounts.get(to).unwrap().balance;
            recipient_balance.checked_add(amount).ok_or_else(|| {
                OpolysError::InvalidParams(format!(
                    "Recipient balance overflow when crediting {} flakes to {}",
                    amount,
                    to.to_hex()
                ))
            })?;
        }

        // Debit the sender (amount + fee) and increment nonce for replay protection.
        let from_balance_before = self.accounts.get(from).unwrap().balance;
        self.accounts.get_mut(from).unwrap().balance = from_balance_before - total_needed;
        self.accounts.get_mut(from).unwrap().nonce = from_nonce + 1;

        // Credit only the transfer amount to the recipient; the fee is burned.
        let recipient_balance = self.accounts.get(to).unwrap().balance;
        self.accounts.get_mut(to).unwrap().balance =
            recipient_balance.checked_add(amount).ok_or_else(|| {
                OpolysError::InvalidParams(format!(
                    "Recipient balance overflow when crediting {} flakes to {}",
                    amount,
                    to.to_hex()
                ))
            })?;

        Ok(TransferResult {
            amount,
            fee_burned: fee,
            new_nonce: from_nonce + 1,
        })
    }

    /// Returns the total number of accounts in the store.
    pub fn account_count(&self) -> usize {
        self.accounts.len()
    }

    /// Compute a deterministic Blake3-256 state root hash over all accounts.
    ///
    /// Accounts are sorted by ObjectId to ensure deterministic output across
    /// nodes. Each account is serialized using Borsh and streamed into the
    /// hasher. The resulting hash captures the complete account state:
    /// balances, nonces, and public keys.
    pub fn compute_state_root(&self) -> opolys_core::Hash {
        let mut sorted_ids: Vec<&ObjectId> = self.accounts.keys().collect();
        sorted_ids.sort_by(|a, b| a.0.0.cmp(&b.0.0));

        let mut hasher = Blake3Hasher::new();
        hasher.update(DOMAIN_STATE_ROOT);
        hasher.update(b"accounts");
        for id in sorted_ids {
            if let Some(account) = self.accounts.get(id) {
                let bytes = borsh::to_vec(account)
                    .expect("Account serialization must not fail; this is a consensus bug");
                hasher.update(&bytes);
            }
        }
        hasher.finalize()
    }
}

/// Result of a successful transfer between two accounts.
///
/// The `fee_burned` field records how many flakes were permanently removed
/// from circulation, enforcing Opolys' market-driven fee-burn model.
#[derive(Debug, Clone)]
pub struct TransferResult {
    /// Amount of flakes transferred to the recipient.
    pub amount: FlakeAmount,
    /// Fees burned (removed from circulating supply entirely).
    pub fee_burned: FlakeAmount,
    /// The sender's new nonce after this transaction.
    pub new_nonce: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use opolys_crypto::hash_to_object_id;

    fn test_id(seed: &[u8]) -> ObjectId {
        hash_to_object_id(seed)
    }

    #[test]
    fn create_account() {
        let mut store = AccountStore::new();
        let id = test_id(b"alice");
        store.create_account(id.clone()).unwrap();
        let account = store.get_account(&id).unwrap();
        assert_eq!(account.balance, 0);
        assert_eq!(account.nonce, 0);
    }

    #[test]
    fn credit_and_debit() {
        let mut store = AccountStore::new();
        let id = test_id(b"alice");
        store.create_account(id.clone()).unwrap();
        store.credit(&id, 1000).unwrap();
        assert_eq!(store.get_account(&id).unwrap().balance, 1000);
        store.debit(&id, 500).unwrap();
        assert_eq!(store.get_account(&id).unwrap().balance, 500);
    }

    #[test]
    fn debit_insufficient_fails() {
        let mut store = AccountStore::new();
        let id = test_id(b"alice");
        store.create_account(id.clone()).unwrap();
        store.credit(&id, 100).unwrap();
        assert!(store.debit(&id, 200).is_err());
    }

    #[test]
    fn transfer_success() {
        let mut store = AccountStore::new();
        let alice = test_id(b"alice");
        let bob = test_id(b"bob");
        store.create_account(alice.clone()).unwrap();
        store.create_account(bob.clone()).unwrap();
        store.credit(&alice, 10_000).unwrap();

        let result = store.transfer(&alice, &bob, 1000, 100).unwrap();
        assert_eq!(result.amount, 1000);
        assert_eq!(result.fee_burned, 100);
        assert_eq!(result.new_nonce, 1);
        assert_eq!(store.get_account(&alice).unwrap().balance, 8900);
        assert_eq!(store.get_account(&bob).unwrap().balance, 1000);
    }

    #[test]
    fn transfer_auto_creates_recipient() {
        let mut store = AccountStore::new();
        let alice = test_id(b"alice");
        let bob = test_id(b"bob");
        store.create_account(alice.clone()).unwrap();
        store.credit(&alice, 10_000).unwrap();

        store.transfer(&alice, &bob, 1000, 100).unwrap();
        assert!(store.get_account(&bob).is_some());
        assert_eq!(store.get_account(&bob).unwrap().balance, 1000);
    }

    #[test]
    fn transfer_insufficient_fails() {
        let mut store = AccountStore::new();
        let alice = test_id(b"alice");
        let bob = test_id(b"bob");
        store.create_account(alice.clone()).unwrap();
        store.create_account(bob.clone()).unwrap();
        store.credit(&alice, 100).unwrap();

        assert!(store.transfer(&alice, &bob, 200, 0).is_err());
    }

    #[test]
    fn credit_overflow_fails() {
        let mut store = AccountStore::new();
        let alice = test_id(b"alice");
        store.create_account(alice.clone()).unwrap();
        store.credit(&alice, u64::MAX).unwrap();

        assert!(store.credit(&alice, 1).is_err());
    }

    #[test]
    fn transfer_total_cost_overflow_fails() {
        let mut store = AccountStore::new();
        let alice = test_id(b"alice");
        let bob = test_id(b"bob");
        store.create_account(alice.clone()).unwrap();
        store.credit(&alice, u64::MAX).unwrap();

        assert!(store.transfer(&alice, &bob, u64::MAX, 1).is_err());
        assert_eq!(store.get_account(&alice).unwrap().balance, u64::MAX);
        assert_eq!(store.get_account(&alice).unwrap().nonce, 0);
    }

    #[test]
    fn transfer_recipient_overflow_fails_without_debit() {
        let mut store = AccountStore::new();
        let alice = test_id(b"alice");
        let bob = test_id(b"bob");
        store.create_account(alice.clone()).unwrap();
        store.create_account(bob.clone()).unwrap();
        store.credit(&alice, 100).unwrap();
        store.credit(&bob, u64::MAX).unwrap();

        assert!(store.transfer(&alice, &bob, 1, 1).is_err());
        assert_eq!(store.get_account(&alice).unwrap().balance, 100);
        assert_eq!(store.get_account(&alice).unwrap().nonce, 0);
        assert_eq!(store.get_account(&bob).unwrap().balance, u64::MAX);
    }

    #[test]
    fn transfer_nonce_overflow_fails_without_side_effects() {
        let mut store = AccountStore::new();
        let alice = test_id(b"alice");
        let bob = test_id(b"bob");
        store.create_account(alice.clone()).unwrap();
        store.credit(&alice, 100).unwrap();
        store.get_account_mut(&alice).unwrap().nonce = u64::MAX;

        assert!(store.transfer(&alice, &bob, 1, 1).is_err());
        assert_eq!(store.get_account(&alice).unwrap().balance, 100);
        assert_eq!(store.get_account(&alice).unwrap().nonce, u64::MAX);
        assert!(store.get_account(&bob).is_none());
    }
}
