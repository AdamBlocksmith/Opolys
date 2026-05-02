//! RocksDB-backed persistent storage for the Opolys blockchain.
//!
//! The `BlockchainStore` provides durable storage for blocks, accounts,
//! refiners, and chain state. It uses RocksDB column families to
//! partition data:
//!
//! | Column Family | Key                      | Value                                  |
//! |----------------|--------------------------|----------------------------------------|
//! | `blocks`       | height (big-endian)      | serialized `Block`                     |
//! | `accounts`     | `"all_accounts"`         | serialized `Vec<Account>`              |
//! | `refiners`   | `"refiner:{hex_id}"`   | Borsh-serialized `RefinerInfo`       |
//! | `refiners`   | `"active_refiner_ids"` | Borsh-serialized `Vec<ObjectId>`       |
//! | `refiners`   | `"refiner_count"`      | u64 little-endian total count          |
//! | `refiners`   | `"unbonding_queue"`      | Borsh-serialized `Vec<PendingUnbond>`  |
//! | `chain_state`  | `"chain_state"`          | serialized `PersistedChainState`       |
//! | `chain_state`  | `"latest_block_height"`  | height (big-endian)                    |
//!
//! Serialization uses [Borsh](https://borsh.io) for deterministic binary encoding.
//! Compression is enabled with LZ4 to reduce disk footprint.

use borsh::{BorshDeserialize, BorshSerialize};
use opolys_consensus::account::{Account, AccountStore};
use opolys_consensus::refiner::{PendingUnbond, RefinerInfo, RefinerSet};
use opolys_core::{Block, Hash, ObjectId, Transaction};
use rocksdb::{WriteBatch, WriteOptions};
use std::path::Path;

/// Serializable snapshot of chain state persisted across node restarts.
///
/// Contains all information needed to resume the node from where it left off
/// without re-processing the entire chain. Difficulty and issuance are
/// emergent — they are computed from chain state, not from governance.
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct PersistedChainState {
    /// Network chain id for the state in this database.
    ///
    /// Nodes must refuse to load state whose chain_id does not match the
    /// configured network. This prevents accidentally mixing testnet/mainnet
    /// balances when data directories are reused.
    pub chain_id: u64,
    /// Blake3-256 hash of the genesis block header for this database.
    ///
    /// Mainnet identity is the chain id plus the exact ceremony-derived
    /// genesis hash. Nodes must refuse to load state from a different
    /// ceremony, even if both states use MAINNET_CHAIN_ID.
    pub genesis_hash: [u8; 32],
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
    /// Suggested fee for the next block (Flakes), computed via EMA.
    /// Starts at MIN_FEE (1 Flake) and adjusts based on network demand.
    pub suggested_fee: u64,
    /// Ceremony-derived block reward in Flakes. Zero on pre-ceremony builds
    /// (migration: node falls back to the BASE_REWARD constant on load).
    pub base_reward: u64,
    /// Persisted double-sign detection map: (height, producer_hex, block_hash, signature).
    /// Survives node restarts so evidence can still be built after a reboot.
    pub producer_signatures: Vec<(u64, String, Hash, Vec<u8>)>,
    /// Height of the latest finalized block.
    /// 0 means no block is finalized yet. Placeholder until finality via attestations (Pass 2).
    pub finalized_height: u64,
}

/// Persistent storage backed by RocksDB.
///
/// Stores blocks, accounts, refiners, and chain state in column families.
/// On startup the node loads persisted state; after each block it saves
/// all state atomically. Uses LZ4 compression to reduce disk usage.
pub struct BlockchainStore {
    db: rocksdb::DB,
}

const VALUE_CHECKSUM_LEN: usize = 16;

fn encode_value(data: &[u8]) -> Vec<u8> {
    let checksum = opolys_crypto::hash::hash_with_domain(b"OPL_DB_VALUE_V1", data);
    let mut encoded = Vec::with_capacity(VALUE_CHECKSUM_LEN + data.len());
    encoded.extend_from_slice(&checksum.as_bytes()[..VALUE_CHECKSUM_LEN]);
    encoded.extend_from_slice(data);
    encoded
}

fn decode_value(context: &str, data: &[u8]) -> Result<Vec<u8>, String> {
    if data.len() < VALUE_CHECKSUM_LEN {
        return Err(format!("{} checksum missing", context));
    }
    let (stored_checksum, payload) = data.split_at(VALUE_CHECKSUM_LEN);
    let computed = opolys_crypto::hash::hash_with_domain(b"OPL_DB_VALUE_V1", payload);
    if stored_checksum != &computed.as_bytes()[..VALUE_CHECKSUM_LEN] {
        return Err(format!("{} checksum mismatch", context));
    }
    Ok(payload.to_vec())
}

fn synced_write_options() -> WriteOptions {
    let mut opts = WriteOptions::default();
    opts.set_sync(true);
    opts
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
            "refiners",
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
        let data =
            borsh::to_vec(block).map_err(|e| format!("Block serialization failed: {}", e))?;

        let cf = self
            .db
            .cf_handle("blocks")
            .ok_or_else(|| "Column family 'blocks' not found".to_string())?;

        self.db
            .put_cf(&cf, &height_key, encode_value(&data))
            .map_err(|e| format!("Block put failed: {}", e))?;

        // Store the height of the latest block for quick lookup
        let state_cf = self
            .db
            .cf_handle("chain_state")
            .ok_or_else(|| "Column family 'chain_state' not found".to_string())?;

        self.db
            .put_cf(&state_cf, b"latest_block_height", encode_value(&height_key))
            .map_err(|e| format!("Latest block height put failed: {}", e))?;

        Ok(())
    }

    /// Load a block by height.
    pub fn load_block(&self, height: u64) -> Result<Option<Block>, String> {
        let height_key = height.to_be_bytes();
        let cf = self
            .db
            .cf_handle("blocks")
            .ok_or_else(|| "Column family 'blocks' not found".to_string())?;

        match self.db.get_cf(&cf, &height_key) {
            Ok(Some(data)) => {
                let payload = decode_value("Block", &data)?;
                let block: Block = Block::try_from_slice(&payload)
                    .map_err(|e| format!("Block deserialization failed: {}", e))?;
                Ok(Some(block))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(format!("Block get failed: {}", e)),
        }
    }

    /// Get the height of the latest persisted block, if any.
    pub fn latest_block_height(&self) -> Result<Option<u64>, String> {
        let cf = self
            .db
            .cf_handle("chain_state")
            .ok_or_else(|| "Column family 'chain_state' not found".to_string())?;

        match self.db.get_cf(&cf, b"latest_block_height") {
            Ok(Some(data)) => {
                let payload = decode_value("Latest block height", &data)?;
                let height = u64::from_be_bytes(
                    payload
                        .as_slice()
                        .try_into()
                        .map_err(|_| "Invalid latest block height")?,
                );
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
        let blocks_cf = self
            .db
            .cf_handle("blocks")
            .ok_or_else(|| "Column family 'blocks' not found".to_string())?;
        let hash_key = format!("hash_{}", block_hash.to_hex());
        self.db
            .put_cf(
                &blocks_cf,
                hash_key.as_bytes(),
                encode_value(&block.header.height.to_be_bytes()),
            )
            .map_err(|e| format!("Block hash index put failed: {}", e))?;

        // tx_id → (height, index) for each transaction in the block
        let tx_cf = self
            .db
            .cf_handle("transactions")
            .ok_or_else(|| "Column family 'transactions' not found".to_string())?;
        for (idx, tx) in block.transactions.iter().enumerate() {
            let mut val = Vec::new();
            val.extend_from_slice(&block.header.height.to_be_bytes());
            val.extend_from_slice(&(idx as u32).to_be_bytes());
            let tx_key = format!("tx_{}", tx.tx_id.to_hex());
            self.db
                .put_cf(&tx_cf, tx_key.as_bytes(), encode_value(&val))
                .map_err(|e| format!("Tx index put failed: {}", e))?;
        }

        Ok(())
    }

    /// Atomically persist a newly applied block and all state derived from it.
    pub fn save_applied_block_atomic(
        &self,
        block: &Block,
        state: &PersistedChainState,
        accounts: &AccountStore,
        refiners: &RefinerSet,
    ) -> Result<(), String> {
        let blocks_cf = self
            .db
            .cf_handle("blocks")
            .ok_or_else(|| "Column family 'blocks' not found".to_string())?;
        let accounts_cf = self
            .db
            .cf_handle("accounts")
            .ok_or_else(|| "Column family 'accounts' not found".to_string())?;
        let refiners_cf = self
            .db
            .cf_handle("refiners")
            .ok_or_else(|| "Column family 'refiners' not found".to_string())?;
        let chain_cf = self
            .db
            .cf_handle("chain_state")
            .ok_or_else(|| "Column family 'chain_state' not found".to_string())?;
        let tx_cf = self
            .db
            .cf_handle("transactions")
            .ok_or_else(|| "Column family 'transactions' not found".to_string())?;

        let height_key = block.header.height.to_be_bytes();
        let block_data =
            borsh::to_vec(block).map_err(|e| format!("Block serialization failed: {}", e))?;
        let chain_data =
            borsh::to_vec(state).map_err(|e| format!("Chain state serialization failed: {}", e))?;
        let accounts_data = borsh::to_vec(&accounts.all_accounts())
            .map_err(|e| format!("Account serialization failed: {}", e))?;

        let all_refiners = refiners.all_refiners();
        let refiner_count = all_refiners.len() as u64;
        let all_refiners_data = borsh::to_vec(&all_refiners)
            .map_err(|e| format!("Refiner serialization failed: {}", e))?;
        let active_data = borsh::to_vec(&refiners.active_set_ids().clone())
            .map_err(|e| format!("Active set serialization failed: {}", e))?;
        let queue_data = borsh::to_vec(&refiners.unbonding_queue)
            .map_err(|e| format!("Unbonding queue serialization failed: {}", e))?;

        let block_hash = opolys_consensus::block::compute_block_hash(&block.header);
        let hash_key = format!("hash_{}", block_hash.to_hex());

        let mut batch = WriteBatch::default();
        batch.put_cf(&blocks_cf, &height_key, encode_value(&block_data));
        batch.put_cf(&blocks_cf, hash_key.as_bytes(), encode_value(&height_key));
        batch.put_cf(&chain_cf, b"latest_block_height", encode_value(&height_key));
        batch.put_cf(&chain_cf, b"chain_state", encode_value(&chain_data));
        batch.put_cf(&accounts_cf, b"all_accounts", encode_value(&accounts_data));

        for (idx, tx) in block.transactions.iter().enumerate() {
            let mut val = Vec::new();
            val.extend_from_slice(&block.header.height.to_be_bytes());
            val.extend_from_slice(&(idx as u32).to_be_bytes());
            let tx_key = format!("tx_{}", tx.tx_id.to_hex());
            batch.put_cf(&tx_cf, tx_key.as_bytes(), encode_value(&val));
        }

        for refiner in &all_refiners {
            let key = format!("refiner:{}", refiner.object_id.to_hex());
            let data = borsh::to_vec(refiner)
                .map_err(|e| format!("Refiner serialization failed: {}", e))?;
            batch.put_cf(&refiners_cf, key.as_bytes(), encode_value(&data));
        }
        batch.put_cf(
            &refiners_cf,
            b"active_refiner_ids",
            encode_value(&active_data),
        );
        batch.put_cf(
            &refiners_cf,
            b"refiner_count",
            encode_value(&refiner_count.to_le_bytes()),
        );
        batch.put_cf(
            &refiners_cf,
            b"all_refiners",
            encode_value(&all_refiners_data),
        );
        batch.put_cf(&refiners_cf, b"unbonding_queue", encode_value(&queue_data));

        self.db
            .write_opt(batch, &synced_write_options())
            .map_err(|e| format!("Atomic block/state write failed: {}", e))?;

        Ok(())
    }

    /// Load a block by its Blake3 hash (hex-encoded).
    pub fn load_block_by_hash(&self, hash: &Hash) -> Result<Option<Block>, String> {
        let blocks_cf = self
            .db
            .cf_handle("blocks")
            .ok_or_else(|| "Column family 'blocks' not found".to_string())?;
        let hash_key = format!("hash_{}", hash.to_hex());

        match self.db.get_cf(&blocks_cf, hash_key.as_bytes()) {
            Ok(Some(data)) => {
                let payload = decode_value("Block hash index", &data)?;
                let height = u64::from_be_bytes(
                    payload
                        .as_slice()
                        .try_into()
                        .map_err(|_| "Invalid height in hash index")?,
                );
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
        let tx_cf = self
            .db
            .cf_handle("transactions")
            .ok_or_else(|| "Column family 'transactions' not found".to_string())?;
        let tx_key = format!("tx_{}", tx_id.to_hex());

        match self.db.get_cf(&tx_cf, tx_key.as_bytes()) {
            Ok(Some(data)) => {
                let payload = decode_value("Transaction index", &data)?;
                if payload.len() < 12 {
                    return Err("Invalid transaction index data".to_string());
                }
                let height = u64::from_be_bytes(
                    payload[..8]
                        .try_into()
                        .map_err(|_| "Invalid height in tx index")?,
                );
                let _index = u32::from_be_bytes(
                    payload[8..12]
                        .try_into()
                        .map_err(|_| "Invalid index in tx index")?,
                );

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
        let cf = self
            .db
            .cf_handle("accounts")
            .ok_or_else(|| "Column family 'accounts' not found".to_string())?;

        let all_accounts = accounts.all_accounts();
        let data = borsh::to_vec(&all_accounts)
            .map_err(|e| format!("Account serialization failed: {}", e))?;

        self.db
            .put_cf(&cf, b"all_accounts", encode_value(&data))
            .map_err(|e| format!("Account put failed: {}", e))?;

        Ok(())
    }

    /// Load all accounts from disk. Returns a fresh AccountStore if none exist.
    pub fn load_accounts(&self) -> Result<AccountStore, String> {
        let cf = self
            .db
            .cf_handle("accounts")
            .ok_or_else(|| "Column family 'accounts' not found".to_string())?;

        match self.db.get_cf(&cf, b"all_accounts") {
            Ok(Some(data)) => {
                let payload = decode_value("Accounts", &data)?;
                let accounts: Vec<Account> = Vec::<Account>::try_from_slice(&payload)
                    .map_err(|e| format!("Account deserialization failed: {}", e))?;
                Ok(AccountStore::load_from_accounts(accounts))
            }
            Ok(None) => Ok(AccountStore::new()),
            Err(e) => Err(format!("Account get failed: {}", e)),
        }
    }

    // ─── Refiner storage ───────────────────────────────────────────

    /// Save all refiners individually and update the active-set index.
    ///
    /// Each refiner is stored under key `"refiner:{hex_object_id}"`.
    /// The active set is separately stored as `"active_refiner_ids"` for
    /// fast O(active_set) startup load, and `"refiner_count"` tracks total.
    pub fn save_refiners(&self, refiners: &RefinerSet) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle("refiners")
            .ok_or_else(|| "Column family 'refiners' not found".to_string())?;

        // Write each refiner individually
        let all_refiners = refiners.all_refiners();
        let count = all_refiners.len() as u64;
        for v in &all_refiners {
            let key = format!("refiner:{}", v.object_id.to_hex());
            let data =
                borsh::to_vec(v).map_err(|e| format!("Refiner serialization failed: {}", e))?;
            self.db
                .put_cf(&cf, key.as_bytes(), encode_value(&data))
                .map_err(|e| format!("Refiner put failed: {}", e))?;
        }

        // Write active set index for fast startup
        let active_ids = refiners.active_set_ids().clone();
        let active_data = borsh::to_vec(&active_ids)
            .map_err(|e| format!("Active set serialization failed: {}", e))?;
        self.db
            .put_cf(&cf, b"active_refiner_ids", encode_value(&active_data))
            .map_err(|e| format!("Active set put failed: {}", e))?;

        // Write total count
        self.db
            .put_cf(&cf, b"refiner_count", encode_value(&count.to_le_bytes()))
            .map_err(|e| format!("Refiner count put failed: {}", e))?;

        // Also keep backward-compatible "all_refiners" blob for migration
        let data = borsh::to_vec(&all_refiners)
            .map_err(|e| format!("Refiner serialization failed: {}", e))?;
        self.db
            .put_cf(&cf, b"all_refiners", encode_value(&data))
            .map_err(|e| format!("Refiner put failed: {}", e))?;

        // Persist the unbonding queue
        let queue_data = borsh::to_vec(&refiners.unbonding_queue)
            .map_err(|e| format!("Unbonding queue serialization failed: {}", e))?;
        self.db
            .put_cf(&cf, b"unbonding_queue", encode_value(&queue_data))
            .map_err(|e| format!("Unbonding queue put failed: {}", e))?;

        Ok(())
    }

    /// Save only the refiners marked dirty since the last full save.
    ///
    /// Incremental save: only writes changed refiners. Caller must pass
    /// the dirty set from `RefinerSet::dirty_refiners`.
    pub fn save_dirty_refiners(
        &self,
        refiners: &RefinerSet,
        dirty_ids: &std::collections::HashSet<ObjectId>,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle("refiners")
            .ok_or_else(|| "Column family 'refiners' not found".to_string())?;

        for id in dirty_ids {
            if let Some(v) = refiners.get_refiner(id) {
                let key = format!("refiner:{}", v.object_id.to_hex());
                let data =
                    borsh::to_vec(v).map_err(|e| format!("Refiner serialization failed: {}", e))?;
                self.db
                    .put_cf(&cf, key.as_bytes(), encode_value(&data))
                    .map_err(|e| format!("Refiner put failed: {}", e))?;
            }
        }

        // Always update active set index when saving dirty refiners
        let active_ids = refiners.active_set_ids().clone();
        let active_data = borsh::to_vec(&active_ids)
            .map_err(|e| format!("Active set serialization failed: {}", e))?;
        self.db
            .put_cf(&cf, b"active_refiner_ids", encode_value(&active_data))
            .map_err(|e| format!("Active set index put failed: {}", e))?;

        Ok(())
    }

    /// Load all refiners and unbonding queue from disk. Returns a fresh RefinerSet if none exist.
    pub fn load_refiners(&self) -> Result<RefinerSet, String> {
        let cf = self
            .db
            .cf_handle("refiners")
            .ok_or_else(|| "Column family 'refiners' not found".to_string())?;

        // Load from the blob (supports both old and new storage formats)
        let refiners = match self.db.get_cf(&cf, b"all_refiners") {
            Ok(Some(data)) => {
                let payload = decode_value("Refiners", &data)?;
                Vec::<RefinerInfo>::try_from_slice(&payload)
                    .map_err(|e| format!("Refiner deserialization failed: {}", e))?
            }
            Ok(None) => vec![],
            Err(e) => return Err(format!("Refiner get failed: {}", e)),
        };

        let unbonding_queue = match self.db.get_cf(&cf, b"unbonding_queue") {
            Ok(Some(data)) => {
                let payload = decode_value("Unbonding queue", &data)?;
                Vec::<PendingUnbond>::try_from_slice(&payload)
                    .map_err(|e| format!("Unbonding queue deserialization failed: {}", e))?
            }
            Ok(None) => vec![],
            Err(e) => return Err(format!("Unbonding queue get failed: {}", e)),
        };

        Ok(RefinerSet::load_from_refiners(refiners, unbonding_queue))
    }

    // ─── Chain state storage ──────────────────────────────────────────

    /// Save chain state (height, difficulty, issued/burned totals, etc.)
    pub fn save_chain_state(&self, state: &PersistedChainState) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle("chain_state")
            .ok_or_else(|| "Column family 'chain_state' not found".to_string())?;

        let data =
            borsh::to_vec(state).map_err(|e| format!("Chain state serialization failed: {}", e))?;

        self.db
            .put_cf(&cf, b"chain_state", encode_value(&data))
            .map_err(|e| format!("Chain state put failed: {}", e))?;

        Ok(())
    }

    /// Load chain state from disk. Returns None if no state exists (fresh database).
    pub fn load_chain_state(&self) -> Result<Option<PersistedChainState>, String> {
        let cf = self
            .db
            .cf_handle("chain_state")
            .ok_or_else(|| "Column family 'chain_state' not found".to_string())?;

        match self.db.get_cf(&cf, b"chain_state") {
            Ok(Some(data)) => {
                let payload = decode_value("Chain state", &data)?;
                let state = PersistedChainState::try_from_slice(&payload)
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
            chain_id: opolys_core::MAINNET_CHAIN_ID,
            genesis_hash: [7u8; 32],
            current_height: 42,
            current_difficulty: 100,
            total_issued: 500_000_000,
            total_burned: 1_000_000,
            block_timestamps: vec![1000, 1120, 1240],
            latest_block_hash: [99u8; 32],
            state_root: [0u8; 32],
            suggested_fee: 1,
            base_reward: 332_000_000,
            producer_signatures: vec![],
            finalized_height: 0,
        };

        store.save_chain_state(&state).unwrap();
        let loaded = store.load_chain_state().unwrap().unwrap();
        assert_eq!(loaded.chain_id, opolys_core::MAINNET_CHAIN_ID);
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
    fn save_and_load_refiners() {
        let dir = temp_dir();
        let store = BlockchainStore::open(dir.path()).unwrap();

        let mut refiners = RefinerSet::new();
        let id = opolys_crypto::hash_to_object_id(b"refiner1");
        refiners
            .bond(id.clone(), opolys_core::MIN_BOND_STAKE, 0, 0)
            .unwrap();

        store.save_refiners(&refiners).unwrap();
        let loaded = store.load_refiners().unwrap();

        assert_eq!(loaded.refiner_count(), 1);
        assert_eq!(
            loaded.get_refiner(&id).unwrap().total_stake(),
            opolys_core::MIN_BOND_STAKE
        );
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
    fn atomic_block_commit_writes_state_and_indexes() {
        let dir = temp_dir();
        let store = BlockchainStore::open(dir.path()).unwrap();

        let genesis_config = GenesisConfig::default();
        let block = opolys_consensus::build_genesis_block(&genesis_config);
        let block_hash = opolys_consensus::block::compute_block_hash(&block.header);

        let mut accounts = AccountStore::new();
        let account_id = opolys_crypto::hash_to_object_id(b"atomic-account");
        accounts.create_account(account_id.clone()).unwrap();
        accounts.credit(&account_id, 1_000_000).unwrap();

        let mut refiners = RefinerSet::new();
        let refiner_id = opolys_crypto::hash_to_object_id(b"atomic-refiner");
        refiners
            .bond(refiner_id.clone(), opolys_core::MIN_BOND_STAKE, 0, 0)
            .unwrap();

        let state = PersistedChainState {
            chain_id: opolys_core::MAINNET_CHAIN_ID,
            genesis_hash: block_hash.0,
            current_height: block.header.height,
            current_difficulty: block.header.difficulty,
            total_issued: 1_000_000,
            total_burned: 0,
            block_timestamps: vec![block.header.timestamp],
            latest_block_hash: block_hash.0,
            state_root: block.header.state_root.0,
            suggested_fee: block.header.suggested_fee,
            base_reward: genesis_config.base_reward,
            producer_signatures: vec![],
            finalized_height: 0,
        };

        store
            .save_applied_block_atomic(&block, &state, &accounts, &refiners)
            .unwrap();

        assert_eq!(
            store.latest_block_height().unwrap(),
            Some(block.header.height)
        );
        assert!(store.load_block_by_hash(&block_hash).unwrap().is_some());
        assert_eq!(
            store.load_chain_state().unwrap().unwrap().current_height,
            block.header.height
        );
        assert_eq!(
            store
                .load_accounts()
                .unwrap()
                .get_account(&account_id)
                .unwrap()
                .balance,
            1_000_000
        );
        assert_eq!(
            store
                .load_refiners()
                .unwrap()
                .get_refiner(&refiner_id)
                .unwrap()
                .total_stake(),
            opolys_core::MIN_BOND_STAKE
        );
    }

    #[test]
    fn corrupted_chain_state_checksum_is_rejected() {
        let dir = temp_dir();
        let store = BlockchainStore::open(dir.path()).unwrap();

        let state = PersistedChainState {
            chain_id: opolys_core::MAINNET_CHAIN_ID,
            genesis_hash: [3u8; 32],
            current_height: 1,
            current_difficulty: 1,
            total_issued: 0,
            total_burned: 0,
            block_timestamps: vec![1000],
            latest_block_hash: [1u8; 32],
            state_root: [2u8; 32],
            suggested_fee: 1,
            base_reward: 332_000_000,
            producer_signatures: vec![],
            finalized_height: 0,
        };

        store.save_chain_state(&state).unwrap();

        let cf = store.db.cf_handle("chain_state").unwrap();
        let mut raw = store.db.get_cf(&cf, b"chain_state").unwrap().unwrap();
        let last = raw.len() - 1;
        raw[last] ^= 0xff;
        store.db.put_cf(&cf, b"chain_state", raw).unwrap();

        let err = store.load_chain_state().unwrap_err();
        assert!(err.contains("checksum mismatch"));
    }

    #[test]
    fn fresh_database_returns_empty_state() {
        let dir = temp_dir();
        let store = BlockchainStore::open(dir.path()).unwrap();

        assert!(store.load_chain_state().unwrap().is_none());
        assert!(store.load_block(0).unwrap().is_none());
        assert_eq!(store.load_accounts().unwrap().account_count(), 0);
        assert_eq!(store.load_refiners().unwrap().refiner_count(), 0);
    }
}
