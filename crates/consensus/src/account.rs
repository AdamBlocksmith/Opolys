use opolys_core::{FleckAmount, ObjectId, OpolysError, Hash};

#[derive(Debug, Clone)]
pub struct Account {
    pub object_id: ObjectId,
    pub balance: FleckAmount,
    pub nonce: u64,
}

impl Account {
    pub fn new(object_id: ObjectId) -> Self {
        Account {
            object_id,
            balance: 0,
            nonce: 0,
        }
    }

    pub fn can_spend(&self, amount: FleckAmount) -> bool {
        self.balance >= amount
    }
}

#[derive(Debug)]
pub struct AccountStore {
    accounts: std::collections::HashMap<ObjectId, Account>,
}

impl AccountStore {
    pub fn new() -> Self {
        AccountStore {
            accounts: std::collections::HashMap::new(),
        }
    }

    pub fn create_account(&mut self, object_id: ObjectId) -> Result<&Account, OpolysError> {
        if self.accounts.contains_key(&object_id) {
            return Err(OpolysError::AccountNotFound(format!("Account already exists: {}", object_id.to_hex())));
        }
        self.accounts.insert(object_id.clone(), Account::new(object_id.clone()));
        Ok(self.accounts.get(&object_id).unwrap())
    }

    pub fn get_account(&self, object_id: &ObjectId) -> Option<&Account> {
        self.accounts.get(object_id)
    }

    pub fn get_account_mut(&mut self, object_id: &ObjectId) -> Option<&mut Account> {
        self.accounts.get_mut(object_id)
    }

    pub fn credit(&mut self, object_id: &ObjectId, amount: FleckAmount) -> Result<(), OpolysError> {
        let account = self.accounts.get_mut(object_id)
            .ok_or_else(|| OpolysError::AccountNotFound(object_id.to_hex()))?;
        account.balance = account.balance.saturating_add(amount);
        Ok(())
    }

    pub fn debit(&mut self, object_id: &ObjectId, amount: FleckAmount) -> Result<(), OpolysError> {
        let account = self.accounts.get_mut(object_id)
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

    pub fn transfer(
        &mut self,
        from: &ObjectId,
        to: &ObjectId,
        amount: FleckAmount,
        fee: FleckAmount,
    ) -> Result<TransferResult, OpolysError> {
        let total_needed = amount.saturating_add(fee);
        let from_account = self.accounts.get(from)
            .ok_or_else(|| OpolysError::AccountNotFound(from.to_hex()))?;

        if from_account.balance < total_needed {
            return Err(OpolysError::InsufficientBalance {
                need: total_needed,
                have: from_account.balance,
            });
        }

        let from_nonce = from_account.nonce;

        let to_exists = self.accounts.contains_key(to);
        if !to_exists {
            self.accounts.insert(to.clone(), Account::new(to.clone()));
        }

        let from_balance_before = self.accounts.get(from).unwrap().balance;
        self.accounts.get_mut(from).unwrap().balance = from_balance_before.saturating_sub(total_needed);
        self.accounts.get_mut(from).unwrap().nonce += 1;
        self.accounts.get_mut(to).unwrap().balance = self.accounts.get(to).unwrap().balance.saturating_add(amount);

        Ok(TransferResult {
            amount,
            fee_burned: fee,
            new_nonce: from_nonce + 1,
        })
    }

    pub fn account_count(&self) -> usize {
        self.accounts.len()
    }
}

#[derive(Debug, Clone)]
pub struct TransferResult {
    pub amount: FleckAmount,
    pub fee_burned: FleckAmount,
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
}