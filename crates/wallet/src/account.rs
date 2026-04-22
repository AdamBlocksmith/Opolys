//! Account management and display formatting for the Opolys wallet.
//!
//! Provides `AccountInfo` (a named account record), `AccountStore` (lookup by
//! object ID or human-readable name), and `format_flake_as_opl` for converting
//! raw flake amounts to human-readable OPL notation (1 OPL = 1,000,000 flakes).

use opolys_core::{ObjectId, FLAKES_PER_OPL};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Metadata for a named wallet account.
///
/// Associates a human-readable name with an on-chain ObjectId
/// and the corresponding ed25519 public key (hex-encoded).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountInfo {
    /// The on-chain identity (Blake3-256 hash of the public key).
    pub object_id: ObjectId,
    /// Human-readable account label (e.g. "alice", "validator-1").
    pub name: String,
    /// Hex-encoded ed25519 public key (64 hex chars = 32 bytes).
    pub public_key_hex: String,
}

/// In-memory store of named wallet accounts.
///
/// Supports lookup by both `ObjectId` (for on-chain operations) and by
/// human-readable name (for CLI/RPC convenience). Serialized to JSON
/// for wallet file persistence.
pub struct AccountStore {
    accounts: HashMap<ObjectId, AccountInfo>,
    name_to_account: HashMap<String, ObjectId>,
}

impl AccountStore {
    /// Create an empty account store.
    pub fn new() -> Self {
        Self {
            accounts: HashMap::new(),
            name_to_account: HashMap::new(),
        }
    }

    /// Register a named account with its public key.
    ///
    /// Creates an `AccountInfo` and indexes it by both ObjectId and name
    /// for O(1) lookup in either direction.
    pub fn add_account(&mut self, name: String, object_id: ObjectId, public_key_hex: String) {
        let info = AccountInfo {
            object_id: object_id.clone(),
            name: name.clone(),
            public_key_hex,
        };
        self.accounts.insert(object_id.clone(), info);
        self.name_to_account.insert(name, object_id);
    }

    /// Look up account metadata by on-chain `ObjectId`.
    pub fn get_by_object_id(&self, object_id: &ObjectId) -> Option<&AccountInfo> {
        self.accounts.get(object_id)
    }

    /// Look up an `ObjectId` by human-readable account name.
    pub fn get_by_name(&self, name: &str) -> Option<&ObjectId> {
        self.name_to_account.get(name)
    }

    /// List all registered accounts.
    pub fn list_accounts(&self) -> Vec<&AccountInfo> {
        self.accounts.values().collect()
    }
}

impl Default for AccountStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Format a flake amount as a human-readable OPL string.
///
/// Opolys uses "flakes" as the on-chain unit (1 OPL = 1,000,000 flakes).
/// This function converts to the format `X.YYYYYY OPL` with 6 decimal places.
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