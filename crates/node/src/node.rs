//! Opolys full-node implementation.
//!
//! The `OpolysNode` orchestrates blockchain state, mining, block application,
//! and persistence. It manages:
//!
//! - **Chain state** — height, difficulty, issuance/burn tracking, block linkage
//! - **Mining** — EVO-OMAP PoW mining loop for block production (parallel by default)
//! - **Block application** — state transitions: transaction execution, fee burning,
//!   reward emission (vein yield), difficulty adjustment, and consensus phase transitions
//! - **Persistence** — saving and loading state via RocksDB
//! - **RPC** — serving chain queries via JSON-RPC
//!
//! Opolys ($OPL) is a blockchain built as decentralized digital gold with no hard cap.
//! Difficulty and rewards emerge from chain state. Fees are market-driven and burned.
//! Validators earn from block rewards only. Only double-signing gets slashed. There
//! is no governance, no schedules, and no fixed percentages.
//!
//! Hashing: Blake3-256 (32 bytes) everywhere. Signatures: ed25519.
//! Key derivation: BIP-39 24-word mnemonics, SLIP-0010 ed25519.

use opolys_core::*;
use opolys_consensus::{
    account::AccountStore,
    emission,
    mempool::Mempool,
    pos::ValidatorSet,
    pow::PowContext,
    genesis::GenesisConfig,
};
use opolys_consensus::difficulty::compute_next_difficulty;
use opolys_consensus::block::{compute_transaction_root, compute_block_hash};
use opolys_consensus::emission::compute_suggested_fee;
use opolys_consensus::pow;
use opolys_execution::TransactionDispatcher;
use opolys_storage::BlockchainStore;
use std::sync::Arc;
use tokio::sync::RwLock;
use clap::Parser;

/// Command-line arguments for the Opolys node.
#[derive(Parser, Debug)]
#[command(name = "opolys", about = "Opolys blockchain node")]
pub struct Args {
    /// P2P listen port (default: 4170).
    #[arg(long, default_value = "4170")]
    pub port: u16,

    /// RPC server port (default: listen_port + 1).
    #[arg(long)]
    pub rpc_port: Option<u16>,

    /// Data directory for RocksDB storage (default: ./data).
    #[arg(long)]
    pub data_dir: Option<String>,

    /// Bootstrap peer address for initial network discovery.
    #[arg(long)]
    pub bootstrap: Option<String>,

    /// Log level: trace, debug, info, warn, error (default: info).
    #[arg(long, default_value = "info")]
    pub log_level: String,

    /// Enable mining loop (default: disabled).
    ///
    /// Without this flag, the node runs in read-only mode — it syncs chain
    /// state and serves RPC queries but does not produce blocks. Pass --mine
    /// to start the EVO-OMAP PoW mining loop.
    #[arg(long)]
    pub mine: bool,

    /// Disable the JSON-RPC server.
    ///
    /// By default, the node listens for JSON-RPC connections on rpc_port.
    /// Pass --no-rpc to skip starting the server (useful for solo mining
    /// without network exposure).
    #[arg(long)]
    pub no_rpc: bool,
}

/// Configuration for an Opolys node, derived from CLI arguments or defaults.
#[derive(Debug, Clone)]
pub struct NodeConfig {
    pub listen_port: u16,
    pub rpc_port: u16,
    pub data_dir: String,
    pub bootstrap_peers: Vec<String>,
    pub log_level: String,
    pub mine: bool,
    pub no_rpc: bool,
}

impl Default for NodeConfig {
    fn default() -> Self {
        NodeConfig {
            listen_port: DEFAULT_LISTEN_PORT,
            rpc_port: DEFAULT_LISTEN_PORT + 1,
            data_dir: "./data".to_string(),
            bootstrap_peers: vec![],
            log_level: "info".to_string(),
            mine: false,
            no_rpc: false,
        }
    }
}

/// Canonical chain state tracking height, difficulty, supply, and consensus phase.
///
/// Difficulty and block rewards emerge from chain state — there are no
/// governance parameters, schedules, or fixed percentages. Fees are
/// market-driven and burned, reducing circulating supply like gold attrition.
#[derive(Debug, Clone)]
pub struct ChainState {
    /// Current block height (0 = genesis).
    pub current_height: u64,
    /// Current mining difficulty — adjusts based on block timestamps and stake.
    pub current_difficulty: u64,
    /// Total OPL flakes emitted across all block rewards (no hard cap).
    pub total_issued: FlakeAmount,
    /// Total OPL flakes permanently removed via fee burning.
    pub total_burned: FlakeAmount,
    /// Rolling window of block timestamps used for difficulty retargeting.
    pub block_timestamps: Vec<u64>,
    /// Blake3-256 hash of the most recent block header.
    pub latest_block_hash: Hash,
    /// Blake3-256 hash of the state root after applying the most recent block.
    pub state_root: Hash,
    /// Current consensus phase — transitions smoothly from PoW to PoS
    /// as stake_coverage increases (no governance, no hard switch).
    pub phase: ConsensusPhase,
    /// Suggested fee for the next block, computed via EMA of previous block's fees.
    /// Starts at MIN_FEE (1 Flake) and adjusts based on network demand.
    pub suggested_fee: FlakeAmount,
}

impl ChainState {
    /// Create chain state from the genesis configuration, computing the
    /// genesis block hash and setting initial values.
    pub fn new(genesis_config: &GenesisConfig) -> Self {
        let genesis = opolys_consensus::build_genesis_block(genesis_config);
        let genesis_hash = compute_block_hash(&genesis.header);

        ChainState {
            current_height: 0,
            current_difficulty: genesis_config.initial_difficulty,
            total_issued: 0,
            total_burned: 0,
            block_timestamps: vec![genesis.header.timestamp],
            latest_block_hash: genesis_hash,
            state_root: genesis.header.state_root.clone(),
            phase: ConsensusPhase::ProofOfWork,
            suggested_fee: MIN_FEE,
        }
    }

    /// Create chain state from persisted data (loaded from RocksDB).
pub fn from_persisted(p: &opolys_storage::PersistedChainState) -> Self {
        let phase = match p.phase {
            0 => ConsensusPhase::ProofOfWork,
            1 => ConsensusPhase::ProofOfStake,
            _ => ConsensusPhase::ProofOfWork,
        };
        ChainState {
            current_height: p.current_height,
            current_difficulty: p.current_difficulty,
            total_issued: p.total_issued,
            total_burned: p.total_burned,
            block_timestamps: p.block_timestamps.clone(),
            latest_block_hash: Hash::from_bytes(p.latest_block_hash),
            state_root: Hash::from_bytes(p.state_root),
            phase,
            suggested_fee: p.suggested_fee,
        }
    }

    /// Convert chain state to the persisted format for storage.
    pub fn to_persisted(&self) -> opolys_storage::PersistedChainState {
        opolys_storage::PersistedChainState {
            current_height: self.current_height,
            current_difficulty: self.current_difficulty,
            total_issued: self.total_issued,
            total_burned: self.total_burned,
            block_timestamps: self.block_timestamps.clone(),
            latest_block_hash: self.latest_block_hash.0,
            state_root: self.state_root.0,
            phase: match self.phase {
                ConsensusPhase::ProofOfWork => 0,
                ConsensusPhase::ProofOfStake => 1,
            },
            suggested_fee: self.suggested_fee,
        }
    }

    /// Circulating supply = total_issued - total_burned.
    pub fn circulating_supply(&self) -> FlakeAmount {
        self.total_issued.saturating_sub(self.total_burned)
    }

    /// Stake coverage = bonded_stake / total_issued.
    pub fn stake_coverage(&self) -> f64 {
        emission::compute_stake_coverage(
            self.total_issued,
            self.total_issued,
        )
    }
}

/// The running Opolys full node.
///
/// Holds all live state behind async `RwLock`s so that the mining loop and
/// RPC handlers can operate concurrently. State is persisted to RocksDB after
/// each block is applied.
pub struct OpolysNode {
    /// Current chain state (height, difficulty, supply, etc.).
    pub chain: Arc<RwLock<ChainState>>,
    /// Live account store (balances, nonces).
    pub accounts: Arc<RwLock<AccountStore>>,
    /// Transaction mempool (sorted by fee).
    pub mempool: Arc<RwLock<Mempool>>,
    /// Live validator set (stake, bonding status).
    pub validators: Arc<RwLock<ValidatorSet>>,
    /// Persistent RocksDB storage (None if running without persistence).
    pub store: Option<Arc<BlockchainStore>>,
    /// Node configuration (ports, data directory, etc.).
    pub config: NodeConfig,
    /// EVO-OMAP mining context with dataset cache for efficient mining.
    pow_context: Arc<RwLock<PowContext>>,
}

impl OpolysNode {
    /// Create a new node, either loading persisted state from disk or
    /// initializing from genesis.
    pub fn new(config: NodeConfig) -> Self {
        let genesis_config = GenesisConfig::default();

        // Try to open the database and load existing state
        let data_path = std::path::PathBuf::from(&config.data_dir);
        let store_result = BlockchainStore::open(&data_path);

        let (chain_state, accounts, validators, store) = match store_result {
            Ok(store) => {
                let store = Arc::new(store);
                match store.load_chain_state() {
                    Ok(Some(persisted)) => {
                        tracing::info!(
                            height = persisted.current_height,
                            difficulty = persisted.current_difficulty,
                            issued = persisted.total_issued,
                            "Loaded persisted chain state from disk"
                        );
                        let chain = ChainState::from_persisted(&persisted);
                        let accs = store.load_accounts().unwrap_or_else(|e| {
                            tracing::warn!("Failed to load accounts, starting fresh: {}", e);
                            AccountStore::new()
                        });
                        let vals = store.load_validators().unwrap_or_else(|e| {
                            tracing::warn!("Failed to load validators, starting fresh: {}", e);
                            ValidatorSet::new()
                        });
                        (chain, accs, vals, Some(store))
                    }
                    Ok(None) => {
                        tracing::info!("No persisted state found, initializing from genesis");
                        let chain = ChainState::new(&genesis_config);
                        (chain, AccountStore::new(), ValidatorSet::new(), Some(store))
                    }
                    Err(e) => {
                        tracing::error!("Failed to load chain state: {}, starting fresh", e);
                        let chain = ChainState::new(&genesis_config);
                        (chain, AccountStore::new(), ValidatorSet::new(), Some(store))
                    }
                }
            }
            Err(e) => {
                tracing::warn!("Could not open database at {:?}: {}, running without persistence", data_path, e);
                let chain = ChainState::new(&genesis_config);
                (chain, AccountStore::new(), ValidatorSet::new(), None)
            }
        };

        OpolysNode {
            chain: Arc::new(RwLock::new(chain_state)),
            accounts: Arc::new(RwLock::new(accounts)),
            mempool: Arc::new(RwLock::new(Mempool::new())),
            validators: Arc::new(RwLock::new(validators)),
            store,
            config,
            pow_context: Arc::new(RwLock::new(PowContext::new())),
        }
    }

    /// Attempt to mine a new block using EVO-OMAP.
    ///
    /// Builds a block header from the current chain state, pulls transactions
    /// from the mempool, computes the transaction root, and runs the EVO-OMAP
    /// PoW mining loop with parallel nonce search. Returns `Some(Block)` if a
    /// valid nonce is found within `max_attempts`, or `None` if the search
    /// is exhausted.
    pub async fn mine_block(&self, max_attempts: u64) -> Option<Block> {
        let chain = self.chain.read().await;
        let accounts = self.accounts.read().await;
        let validators = self.validators.read().await;

        let mempool = self.mempool.read().await;
        let transactions: Vec<Transaction> = mempool.get_ordered_transactions()
            .into_iter()
            .take(100)
            .cloned()
            .collect();

        let transaction_root = compute_transaction_root(&transactions);
        let bonded_stake = validators.total_bonded_stake();
        let total_issued = chain.total_issued;

        let diff_target = compute_next_difficulty(
            chain.current_difficulty,
            chain.current_height,
            &chain.block_timestamps,
            total_issued,
            bonded_stake,
        );

        let difficulty = diff_target.effective_difficulty();

        // Build the block header with all new fields
        let header = BlockHeader {
            version: BLOCK_VERSION,
            height: chain.current_height + 1,
            previous_hash: chain.latest_block_hash.clone(),
            state_root: chain.state_root.clone(),
            transaction_root,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            difficulty,
            suggested_fee: chain.suggested_fee,
            extension_root: None,
            pow_proof: None,
            validator_signature: None,
        };

        drop(chain);
        drop(accounts);
        drop(validators);
        drop(mempool);

        let num_threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);

        let mut ctx = self.pow_context.write().await;
        ctx.mine_parallel(header, difficulty, max_attempts, num_threads)
    }

    /// Apply a mined or received block to the chain state.
    ///
    /// This is the core state transition function:
    /// 1. Validate block (version, height, previous_hash, difficulty, PoW, etc.)
    /// 2. Compute the block hash and update chain linkage
    /// 3. Execute all transactions (Transfer/Bond/Unbond), burning fees
    /// 4. Compute block reward using vein yield (integer-only natural log)
    /// 5. Update issuance, difficulty, suggested_fee, and consensus phase
    /// 6. Remove processed transactions from the mempool
    /// 7. Persist all state to disk (if storage is available)
    pub async fn apply_block(&self, block: &Block) -> Result<Hash, String> {
        let mut chain = self.chain.write().await;
        let mut accounts = self.accounts.write().await;
        let mut validators = self.validators.write().await;
        let mut mempool = self.mempool.write().await;

        let bonded_stake = validators.total_bonded_stake();

        // Compute expected next difficulty for validation
        let expected_difficulty = compute_next_difficulty(
            chain.current_difficulty,
            chain.current_height,
            &chain.block_timestamps,
            chain.total_issued,
            bonded_stake,
        ).effective_difficulty();

        // Compute parent timestamp (0 for genesis)
        let parent_timestamp = chain.block_timestamps.last().copied().unwrap_or(0);
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // Comprehensive block validation
        let expected_height = chain.current_height + 1;
        opolys_consensus::block::validate_block(
            block,
            expected_height,
            &chain.latest_block_hash,
            parent_timestamp,
            expected_difficulty,
            now_secs,
        ).map_err(|e| format!("Block validation failed: {}", e))?;

        // Compute vein yield from the EVO-OMAP PoW hash
        let pow_hash_value = pow::compute_pow_hash_value(&block.header).unwrap_or(0u64);

        // Block reward uses vein yield: BASE_REWARD / difficulty * vein_yield
        let block_reward = emission::compute_block_reward(block.header.difficulty, pow_hash_value);

        // Compute the block hash — this is the new chain tip
        let block_hash = compute_block_hash(&block.header);

        // Compute suggested fee for the next block via EMA
        let total_fees: FlakeAmount = block.transactions.iter().map(|tx| tx.fee).sum();
        let next_suggested_fee = compute_suggested_fee(total_fees, chain.suggested_fee);

        // Update chain state
        chain.total_issued = chain.total_issued.saturating_add(block_reward);
        chain.current_height = block.header.height;
        chain.current_difficulty = block.header.difficulty;
        chain.latest_block_hash = block_hash.clone();
        chain.block_timestamps.push(block.header.timestamp);
        chain.suggested_fee = next_suggested_fee;

        // Execute all transactions in order
        let mut total_fees_burned: FlakeAmount = 0;
        for tx in &block.transactions {
            let result = TransactionDispatcher::apply_transaction(
                tx,
                &mut accounts,
                &mut validators,
                block.header.height,
                block.header.timestamp,
            );
            if result.success {
                total_fees_burned = total_fees_burned.saturating_add(result.fee_burned);
            } else {
                tracing::warn!(
                    tx_id = %tx.tx_id.to_hex(),
                    error = ?result.error,
                    "Transaction failed in block"
                );
            }
            mempool.remove_transaction(&tx.tx_id);
        }

        chain.total_burned = chain.total_burned.saturating_add(total_fees_burned);

        // Update consensus phase based on stake coverage
        if bonded_stake > 0 && emission::compute_stake_coverage(bonded_stake, chain.total_issued) > 0.0 {
            chain.phase = ConsensusPhase::ProofOfStake;
        } else {
            chain.phase = ConsensusPhase::ProofOfWork;
        }

        // Persist state to disk
        if let Some(ref store) = self.store {
            if let Err(e) = Self::persist_state(store, &chain, &accounts, &validators, block) {
                tracing::error!("Failed to persist state: {}", e);
            }
        }

        Ok(block_hash)
    }

    /// Persist all chain state, accounts, validators, and the block to RocksDB.
    fn persist_state(
        store: &BlockchainStore,
        chain: &ChainState,
        accounts: &AccountStore,
        validators: &ValidatorSet,
        block: &Block,
    ) -> Result<(), String> {
        store.save_block(block)?;
        store.save_block_indexes(block)?;
        store.save_chain_state(&chain.to_persisted())?;
        store.save_accounts(accounts)?;
        store.save_validators(validators)?;
        Ok(())
    }

    /// Retrieve a block from storage by height.
    pub fn get_block(&self, height: u64) -> Option<Block> {
        self.store.as_ref()?.load_block(height).ok()?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create a NodeConfig that uses a temporary directory.
    fn test_config() -> (NodeConfig, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("Failed to create temp dir");
        let config = NodeConfig {
            listen_port: 0,
            rpc_port: 0,
            data_dir: dir.path().to_string_lossy().to_string(),
            bootstrap_peers: vec![],
            log_level: "warn".to_string(),
            mine: true,
            no_rpc: true,
        };
        (config, dir)
    }

    #[test]
    fn node_initialization() {
        let (config, _dir) = test_config();
        let node = OpolysNode::new(config);
        assert_eq!(node.chain.blocking_read().current_height, 0);
    }

    #[test]
    fn chain_state_circulating_supply() {
        let genesis_config = GenesisConfig::default();
        let chain = ChainState::new(&genesis_config);
        assert_eq!(chain.circulating_supply(), 0);
    }

    #[test]
    fn chain_state_genesis_hash_is_computed() {
        let config = GenesisConfig::default();
        let chain = ChainState::new(&config);
        assert_ne!(chain.latest_block_hash, Hash::zero());
        assert_eq!(chain.latest_block_hash.to_hex().len(), 64);
    }

    #[test]
    fn chain_state_suggested_fee_starts_at_min() {
        let config = GenesisConfig::default();
        let chain = ChainState::new(&config);
        assert_eq!(chain.suggested_fee, MIN_FEE);
    }

    #[test]
    fn chain_state_persist_roundtrip() {
        let genesis_config = GenesisConfig::default();
        let chain = ChainState::new(&genesis_config);
        let persisted = chain.to_persisted();
        let restored = ChainState::from_persisted(&persisted);
        assert_eq!(restored.current_height, chain.current_height);
        assert_eq!(restored.current_difficulty, chain.current_difficulty);
        assert_eq!(restored.total_issued, chain.total_issued);
        assert_eq!(restored.total_burned, chain.total_burned);
        assert_eq!(restored.latest_block_hash, chain.latest_block_hash);
        assert_eq!(restored.state_root, chain.state_root);
    }

    /// Integration test that mines real EVO-OMAP blocks. Ignored by default
    /// because it takes ~7.5s per hash attempt (requires actual PoW computation).
    /// Run with `cargo test -- --ignored` to include this test.
    #[tokio::test]
    #[ignore]
    async fn mine_and_apply_block_links_chain() {
        let (config, _dir) = test_config();
        let node = OpolysNode::new(config);

        // Capture genesis hash before mining
        let genesis_hash = node.chain.read().await.latest_block_hash.clone();
        assert_ne!(genesis_hash, Hash::zero(), "Genesis hash must be computed, not zero");

        // Mine block 1
        let block = node.mine_block(1_000_000).await.expect("Should mine block 1");
        assert_eq!(block.header.height, 1);
        assert_eq!(block.header.version, BLOCK_VERSION);
        assert_eq!(block.header.previous_hash, genesis_hash, "Block 1 must reference genesis hash");

        // Apply block 1
        let result = node.apply_block(&block).await;
        assert!(result.is_ok(), "Block apply should succeed: {:?}", result);

        let block1_hash = result.unwrap();
        assert_ne!(block1_hash, Hash::zero(), "Block 1 hash must be computed");
        assert_eq!(block1_hash, node.chain.read().await.latest_block_hash);

        // Mine block 2, should reference block 1
        let block2 = node.mine_block(1_000_000).await.expect("Should mine block 2");
        assert_eq!(block2.header.height, 2);
        assert_eq!(block2.header.previous_hash, block1_hash, "Block 2 must reference block 1 hash");
    }
}