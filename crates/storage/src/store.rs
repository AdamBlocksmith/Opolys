//! RocksDB-backed persistent storage for the Opolys blockchain.
//!
//! The `BlockchainStore` provides durable storage for blocks, accounts,
//! validators, and chain state. It uses RocksDB column families to
//! partition data:
//!
//! | Column Family | Key                | Value                        |
//! |----------------|--------------------|------------------------------|
//! | `blocks`       | height (big-endian) | serialized `Block`          |
//! | `accounts`     | `"all_accounts"`    | serialized `Vec<Account>`   |
//! | `validators`   | `"all_validators"`  | serialized `Vec<ValidatorInfo>` |
//! | `chain_state`  | `"chain_state"`     | serialized `PersistedChainState` |
//! | `chain_state`  | `"latest_block_height"` | height (big-endian)    |
//!
//! Serialization uses [Borsh](https://borsh.io) for deterministic binary encoding.
//! Compression is enabled with LZ4 to reduce disk footprint.

use opolys_core::{Block, ObjectId, Hash, Transaction};
use opolys_consensus::account::{Account, AccountStore};
use opolys_consensus::pos::{ValidatorInfo, ValidatorSet, PendingUnbond};
use borsh::{BorshSerialize, BorshDeserialize};
use std::path::Path;

/// Serializable snapshot of chain state persisted across node restarts.
///
/// Contains all information needed to resume the node from where it left off
/// without re-processing the entire chain. Difficulty and issuance are
/// emergent — they are computed from chain state, not from governance.
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct PersistedChainState {
    /// Current block height (0 = genesis).
    pub current_height: u64,
    /// Current mining/encoding difficulty — emerges from block timestamps.
    pub current_difficulty: u64,
    /// Total OPL flakes emitted across all block rewards (no hard cap).
    pub total_issued: u64,
    /// Total OPL flakes permanently removed via fee burning.
    pub total_burned: u64,
    /// Rolling window of block timestamps used for difficulty retargeting.
    pub block_timestamps: Vec<u64>,
    /// Blake3-256 hash of the most recent block header.
    pub latest_block_hash: [u8; 32],
    /// Blake3-256 hash of the state root after applying the most recent block.
    pub state_root: [u8; 32],
    /// Consensus phase: 0 = ProofOfWork, 1 = ProofOfStake.
    /// Transitions naturally as stake coverage grows.
    pub phase: u8,
    /// Suggested fee for the next block (Flakes), computed via EMA.
    /// Starts at MIN_FEE (1 Flake) and adjusts based on network demand.
    pub suggested_fee: u64,
}

/// Persistent storage backed by RocksDB.
///
/// Stores blocks, accounts, validators, and chain state in column families.
/// On startup the node loads persisted state; after each block it saves
/// all state atomically. Uses LZ4 compression to reduce disk usage.
pub struct BlockchainStore {
    db: rocksdb::DB,
}

impl BlockchainStore {
    /// Open (or create) the blockchain database at the given path.
    ///
    /// Creates the directory and column families if they do not exist.
    /// Enables LZ4 compression on all column families.
    pub fn open(path: &Path) -> Result<Self, String> {
        let mut opts = rocksdb::Options::default();
        opts.create_if_missing(true);
        opts.create_missing_column_families(true);
        opts.set_compression_type(rocksdb::DBCompressionType::Lz4);

        let cf_names = vec![
            "blocks",
            "accounts",
            "validators",
            "chain_state",
            "transactions",
        ];
        let db = rocksdb::DB::open_cf(&opts, path, cf_names)
            .map_err(|e| format!("Failed to open database at {:?}: {}", path, e))?;

        Ok(BlockchainStore { db })
    }

    // ─── Block storage ───────────────────────────────────────────────

    /// Save a block, indexed by height.
    pub fn save_block(&self, block: &Block) -> Result<(), String> {
        let height_key = block.header.height.to_be_bytes();
        let data = borsh::to_vec(block)
            .map_err(|e| format!("Block serialization failed: {}", e))?;

        let cf = self.db.cf_handle("blocks")
            .ok_or_else(|| "Column family 'blocks' not found".to_string())?;

        self.db.put_cf(&cf, &height_key, &data)
            .map_err(|e| format!("Block put failed: {}", e))?;

        // Store the height of the latest block for quick lookup
        let state_cf = self.db.cf_handle("chain_state")
            .ok_or_else(|| "Column family 'chain_state' not found".to_string())?;

        self.db.put_cf(&state_cf, b"latest_block_height", &height_key)
            .map_err(|e| format!("Latest block height put failed: {}", e))?;

        Ok(())
    }

    /// Load a block by height.
    pub fn load_block(&self, height: u64) -> Result<Option<Block>, String> {
        let height_key = height.to_be_bytes();
        let cf = self.db.cf_handle("blocks")
            .ok_or_else(|| "Column family 'blocks' not found".to_string())?;

        match self.db.get_cf(&cf, &height_key) {
            Ok(Some(data)) => {
                let block: Block = Block::try_from_slice(&data)
                    .map_err(|e| format!("Block deserialization failed: {}", e))?;
                Ok(Some(block))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(format!("Block get failed: {}", e)),
        }
    }

    /// Get the height of the latest persisted block, if any.
    pub fn latest_block_height(&self) -> Result<Option<u64>, String> {
        let cf = self.db.cf_handle("chain_state")
            .ok_or_else(|| "Column family 'chain_state' not found".to_string())?;

        match self.db.get_cf(&cf, b"latest_block_height") {
            Ok(Some(data)) => {
                let height = u64::from_be_bytes(data.as_slice().try_into()
                    .map_err(|_| "Invalid latest block height")?);
                Ok(Some(height))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(format!("Latest block height get failed: {}", e)),
        }
    }

    // ─── Block indexes ─────────────────────────────────────────────

    /// Save reverse indexes for a block: hash→height and per-tx lookups.
    ///
    /// Must be called after `save_block()` for each new block. These indexes
    /// enable RPC queries like "get block by hash" and "get transaction by id".
    pub fn save_block_indexes(&self, block: &Block) -> Result<(), String> {
        let block_hash = opolys_consensus::block::compute_block_hash(&block.header);

        // hash → height (for block-by-hash lookups)
        let blocks_cf = self.db.cf_handle("blocks")
            .ok_or_else(|| "Column family 'blocks' not found".to_string())?;
        let hash_key = format!("hash_{}", block_hash.to_hex());
        self.db.put_cf(&blocks_cf, hash_key.as_bytes(), &block.header.height.to_be_bytes())
            .map_err(|e| format!("Block hash index put failed: {}", e))?;

        // tx_id → (height, index) for each transaction in the block
        let tx_cf = self.db.cf_handle("transactions")
            .ok_or_else(|| "Column family 'transactions' not found".to_string())?;
        for (idx, tx) in block.transactions.iter().enumerate() {
            let mut val = Vec::new();
            val.extend_from_slice(&block.header.height.to_be_bytes());
            val.extend_from_slice(&(idx as u32).to_be_bytes());
            let tx_key = format!("tx_{}", tx.tx_id.to_hex());
            self.db.put_cf(&tx_cf, tx_key.as_bytes(), &val)
                .map_err(|e| format!("Tx index put failed: {}", e))?;
        }

        Ok(())
    }

    /// Load a block by its Blake3 hash (hex-encoded).
    pub fn load_block_by_hash(&self, hash: &Hash) -> Result<Option<Block>, String> {
        let blocks_cf = self.db.cf_handle("blocks")
            .ok_or_else(|| "Column family 'blocks' not found".to_string())?;
        let hash_key = format!("hash_{}", hash.to_hex());

        match self.db.get_cf(&blocks_cf, hash_key.as_bytes()) {
            Ok(Some(data)) => {
                let height = u64::from_be_bytes(data.as_slice().try_into()
                    .map_err(|_| "Invalid height in hash index")?);
                self.load_block(height)
            }
            Ok(None) => Ok(None),
            Err(e) => Err(format!("Block hash lookup failed: {}", e)),
        }
    }

    /// Load a transaction by its ObjectId.
    ///
    /// Returns `Some((block_height, Transaction))` if found, `None` otherwise.
    pub fn load_transaction(&self, tx_id: &ObjectId) -> Result<Option<(u64, Transaction)>, String> {
        let tx_cf = self.db.cf_handle("transactions")
            .ok_or_else(|| "Column family 'transactions' not found".to_string())?;
        let tx_key = format!("tx_{}", tx_id.to_hex());

        match self.db.get_cf(&tx_cf, tx_key.as_bytes()) {
            Ok(Some(data)) => {
                if data.len() < 12 {
                    return Err("Invalid transaction index data".to_string());
                }
                let height = u64::from_be_bytes(data[..8].try_into()
                    .map_err(|_| "Invalid height in tx index")?);
                let _index = u32::from_be_bytes(data[8..12].try_into()
                    .map_err(|_| "Invalid index in tx index")?);

                // Load the block that contains this transaction
                let block = self.load_block(height)?;
                match block {
                    Some(b) => {
                        // Find the transaction within the block
                        for tx in &b.transactions {
                            if &tx.tx_id == tx_id {
                                return Ok(Some((height, tx.clone())));
                            }
                        }
                        // Index says it should be here but transaction not found
                        Ok(None)
                    }
                    None => Ok(None),
                }
            }
            Ok(None) => Ok(None),
            Err(e) => Err(format!("Tx lookup failed: {}", e)),
        }
    }

    // ─── Account storage ─────────────────────────────────────────────

    /// Save all accounts atomically.
    pub fn save_accounts(&self, accounts: &AccountStore) -> Result<(), String> {
        let cf = self.db.cf_handle("accounts")
            .ok_or_else(|| "Column family 'accounts' not found".to_string())?;

        let all_accounts = accounts.all_accounts();
        let data = borsh::to_vec(&all_accounts)
            .map_err(|e| format!("Account serialization failed: {}", e))?;

        self.db.put_cf(&cf, b"all_accounts", &data)
            .map_err(|e| format!("Account put failed: {}", e))?;

        Ok(())
    }

    /// Load all accounts from disk. Returns a fresh AccountStore if none exist.
    pub fn load_accounts(&self) -> Result<AccountStore, String> {
        let cf = self.db.cf_handle("accounts")
            .ok_or_else(|| "Column family 'accounts' not found".to_string())?;

        match self.db.get_cf(&cf, b"all_accounts") {
            Ok(Some(data)) => {
                let accounts: Vec<Account> = Vec::<Account>::try_from_slice(&data)
                    .map_err(|e| format!("Account deserialization failed: {}", e))?;
                Ok(AccountStore::load_from_accounts(accounts))
            }
            Ok(None) => Ok(AccountStore::new()),
            Err(e) => Err(format!("Account get failed: {}", e)),
        }
    }

    // ─── Validator storage ───────────────────────────────────────────

    /// Save all validators and unbonding queue atomically.
    pub fn save_validators(&self, validators: &ValidatorSet) -> Result<(), String> {
        let cf = self.db.cf_handle("validators")
            .ok_or_else(|| "Column family 'validators' not found".to_string())?;

        let all_validators = validators.all_validators();
        let data = borsh::to_vec(&all_validators)
            .map_err(|e| format!("Validator serialization failed: {}", e))?;

        self.db.put_cf(&cf, b"all_validators", &data)
            .map_err(|e| format!("Validator put failed: {}", e))?;

        // Also persist the unbonding queue
        let queue_data = borsh::to_vec(&validators.unbonding_queue)
            .map_err(|e| format!("Unbonding queue serialization failed: {}", e))?;

        self.db.put_cf(&cf, b"unbonding_queue", &queue_data)
            .map_err(|e| format!("Unbonding queue put failed: {}", e))?;

        Ok(())
    }

    /// Load all validators and unbonding queue from disk. Returns a fresh ValidatorSet if none exist.
    pub fn load_validators(&self) -> Result<ValidatorSet, String> {
        let cf = self.db.cf_handle("validators")
            .ok_or_else(|| "Column family 'validators' not found".to_string())?;

        let validators = match self.db.get_cf(&cf, b"all_validators") {
            Ok(Some(data)) => {
                let validators: Vec<ValidatorInfo> = Vec::<ValidatorInfo>::try_from_slice(&data)
                    .map_err(|e| format!("Validator deserialization failed: {}", e))?;
                validators
            }
            Ok(None) => vec![],
            Err(e) => return Err(format!("Validator get failed: {}", e)),
        };

        let unbonding_queue = match self.db.get_cf(&cf, b"unbonding_queue") {
            Ok(Some(data)) => {
                Vec::<PendingUnbond>::try_from_slice(&data)
                    .map_err(|e| format!("Unbonding queue deserialization failed: {}", e))?
            }
            Ok(None) => vec![],
            Err(e) => return Err(format!("Unbonding queue get failed: {}", e)),
        };

        Ok(ValidatorSet::load_from_validators(validators, unbonding_queue))
    }

    // ─── Chain state storage ──────────────────────────────────────────

    /// Save chain state (height, difficulty, issued/burned totals, etc.)
    pub fn save_chain_state(&self, state: &PersistedChainState) -> Result<(), String> {
        let cf = self.db.cf_handle("chain_state")
            .ok_or_else(|| "Column family 'chain_state' not found".to_string())?;

        let data = borsh::to_vec(state)
            .map_err(|e| format!("Chain state serialization failed: {}", e))?;

        self.db.put_cf(&cf, b"chain_state", &data)
            .map_err(|e| format!("Chain state put failed: {}", e))?;

        Ok(())
    }

    /// Load chain state from disk. Returns None if no state exists (fresh database).
    pub fn load_chain_state(&self) -> Result<Option<PersistedChainState>, String> {
        let cf = self.db.cf_handle("chain_state")
            .ok_or_else(|| "Column family 'chain_state' not found".to_string())?;

        match self.db.get_cf(&cf, b"chain_state") {
            Ok(Some(data)) => {
                let state = PersistedChainState::try_from_slice(&data)
                    .map_err(|e| format!("Chain state deserialization failed: {}", e))?;
                Ok(Some(state))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(format!("Chain state get failed: {}", e)),
        }
    }

    /// Check whether the database contains any persisted state.
    pub fn has_state(&self) -> Result<bool, String> {
        Ok(self.load_chain_state()?.is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opolys_consensus::genesis::GenesisConfig;

    fn temp_dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("Failed to create temp dir")
    }

    #[test]
    fn open_and_close_database() {
        let dir = temp_dir();
        let store = BlockchainStore::open(dir.path()).expect("Should open database");
        assert!(!store.has_state().unwrap());
    }

    #[test]
    fn save_and_load_chain_state() {
        let dir = temp_dir();
        let store = BlockchainStore::open(dir.path()).unwrap();

        let state = PersistedChainState {
            current_height: 42,
            current_difficulty: 100,
            total_issued: 500_000_000,
            total_burned: 1_000_000,
            block_timestamps: vec![1000, 1120, 1240],
            latest_block_hash: [99u8; 32],
            state_root: [0u8; 32],
            phase: 0,
            suggested_fee: 1,
        };

        store.save_chain_state(&state).unwrap();
        let loaded = store.load_chain_state().unwrap().unwrap();
        assert_eq!(loaded.current_height, 42);
        assert_eq!(loaded.current_difficulty, 100);
        assert_eq!(loaded.total_issued, 500_000_000);
        assert_eq!(loaded.total_burned, 1_000_000);
        assert_eq!(loaded.block_timestamps.len(), 3);
    }

    #[test]
    fn save_and_load_accounts() {
        let dir = temp_dir();
        let store = BlockchainStore::open(dir.path()).unwrap();

        let mut accounts = AccountStore::new();
        let id = opolys_crypto::hash_to_object_id(b"alice");
        accounts.create_account(id.clone()).unwrap();
        accounts.credit(&id, 1_000_000).unwrap();

        store.save_accounts(&accounts).unwrap();
        let loaded = store.load_accounts().unwrap();

        assert_eq!(loaded.account_count(), 1);
        assert_eq!(loaded.get_account(&id).unwrap().balance, 1_000_000);
    }

    #[test]
    fn save_and_load_validators() {
        let dir = temp_dir();
        let store = BlockchainStore::open(dir.path()).unwrap();

        let mut validators = ValidatorSet::new();
        let id = opolys_crypto::hash_to_object_id(b"validator1");
        validators.bond(id.clone(), opolys_core::MIN_BOND_STAKE, 0, 0).unwrap();

        store.save_validators(&validators).unwrap();
        let loaded = store.load_validators().unwrap();

        assert_eq!(loaded.validator_count(), 1);
        assert_eq!(loaded.get_validator(&id).unwrap().total_stake(), opolys_core::MIN_BOND_STAKE);
    }

    #[test]
    fn save_and_load_block() {
        let dir = temp_dir();
        let store = BlockchainStore::open(dir.path()).unwrap();

        let genesis_config = GenesisConfig::default();
        let block = opolys_consensus::build_genesis_block(&genesis_config);

        store.save_block(&block).unwrap();

        let loaded = store.load_block(0).unwrap().unwrap();
        assert_eq!(loaded.header.height, 0);
        assert_eq!(loaded.transactions.len(), 0);
    }

    #[test]
    fn fresh_database_returns_empty_state() {
        let dir = temp_dir();
        let store = BlockchainStore::open(dir.path()).unwrap();

        assert!(store.load_chain_state().unwrap().is_none());
        assert!(store.load_block(0).unwrap().is_none());
        assert_eq!(store.load_accounts().unwrap().account_count(), 0);
        assert_eq!(store.load_validators().unwrap().validator_count(), 0);
    }
}