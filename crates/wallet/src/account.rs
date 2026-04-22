use opolys_core::{FlakeAmount, ObjectId, FLAKES_PER_OPL};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountInfo {
    pub object_id: ObjectId,
    pub name: String,
    pub public_key_hex: String,
}

pub struct AccountStore {
    accounts: HashMap<ObjectId, AccountInfo>,
    name_to_account: HashMap<String, ObjectId>,
}

impl AccountStore {
    pub fn new() -> Self {
        Self {
            accounts: HashMap::new(),
            name_to_account: HashMap::new(),
        }
    }

    pub fn add_account(&mut self, name: String, object_id: ObjectId, public_key_hex: String) {
        let info = AccountInfo {
            object_id: object_id.clone(),
            name: name.clone(),
            public_key_hex,
        };
        self.accounts.insert(object_id.clone(), info);
        self.name_to_account.insert(name, object_id);
    }

    pub fn get_by_object_id(&self, object_id: &ObjectId) -> Option<&AccountInfo> {
        self.accounts.get(object_id)
    }

    pub fn get_by_name(&self, name: &str) -> Option<&ObjectId> {
        self.name_to_account.get(name)
    }

    pub fn list_accounts(&self) -> Vec<&AccountInfo> {
        self.accounts.values().collect()
    }
}

impl Default for AccountStore {
    fn default() -> Self {
        Self::new()
    }
}

pub fn format_flake_as_opl(flakes: u64) -> String {
    let opl = flakes / FLAKES_PER_OPL;
    let frac = flakes % FLAKES_PER_OPL;
    format!("{}.{:06} OPL", opl, frac)
}

#[cfg(test)]
mod tests {
    use super::*;
    use opolys_crypto::hash_to_object_id;

    fn test_id(name: &str) -> ObjectId {
        hash_to_object_id(name.as_bytes())
    }

    #[test]
    fn add_and_retrieve_account() {
        let mut store = AccountStore::new();
        let id = test_id("alice");
        store.add_account("alice".into(), id.clone(), "abc123".into());
        assert_eq!(store.get_by_object_id(&id).unwrap().name, "alice");
        assert_eq!(store.get_by_name("alice").unwrap(), &id);
    }

    #[test]
    fn format_flake_amounts() {
        assert_eq!(format_flake_as_opl(1_000_000), "1.000000 OPL");
        assert_eq!(format_flake_as_opl(0), "0.000000 OPL");
        assert_eq!(format_flake_as_opl(1), "0.000001 OPL");
        assert_eq!(format_flake_as_opl(440_000_000), "440.000000 OPL");
    }
}