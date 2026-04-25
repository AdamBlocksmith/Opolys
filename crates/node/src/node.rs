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
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use clap::Parser;
use ed25519_dalek::{Signer, Verifier};

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

    /// Enable validator block production (default: disabled).
    ///
    /// When enabled, the node will produce PoS blocks when it is an active
    /// validator with bonded stake. Requires a wallet key to sign blocks.
    /// This flag is separate from --mine (both can be active simultaneously).
    #[arg(long)]
    pub validate: bool,

    /// Path to the miner/validator key file (32-byte ed25519 seed).
    ///
    /// The ObjectId (Blake3 hash of the public key) derived from this key
    /// is used as the block producer identity. If not provided, the miner_id
    /// defaults to zero (rewards are not credited to any account).
    /// For production use, generate a key with `opl keygen` and provide the path.
    #[arg(long)]
    pub key_file: Option<String>,
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
    pub validate: bool,
    /// Path to the miner/validator key file (32-byte ed25519 seed).
    /// When provided, the node can sign PoS blocks and receive block rewards.
    pub key_file: Option<String>,
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
            validate: false,
            key_file: None,
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
    /// Double-sign detection: tracks which block hash each validator signed at
    /// each height. Key is (height, producer ObjectId hex) → block hash.
    /// If a validator signs a different block at the same height, they are slashed.
    pub producer_signatures: HashMap<(u64, String), Hash>,
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
            producer_signatures: HashMap::new(),
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
            producer_signatures: HashMap::new(),
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
    /// The miner's on-chain identity (Blake3 hash of their public key).
    /// For PoW blocks, this identifies who earns the block reward.
    /// For PoS blocks, this must match an active validator's ObjectId.
    pub miner_id: ObjectId,
    /// The ed25519 signing key for block production. Set when --key-file is provided.
    /// Used by produce_pos_block() to sign PoS blocks.
    pub signing_key: Option<ed25519_dalek::SigningKey>,
}

impl OpolysNode {
    /// Create a new node, either loading persisted state from disk or
    /// initializing from genesis.
    pub fn new(config: NodeConfig) -> Self {
        // Load the miner/validator key from the key file (if provided)
        let (miner_id, signing_key) = if let Some(ref key_path) = config.key_file {
            match std::fs::read(key_path) {
                Ok(seed_bytes) if seed_bytes.len() == 32 => {
                    let mut seed = [0u8; 32];
                    seed.copy_from_slice(&seed_bytes);
                    let sk = ed25519_dalek::SigningKey::from_bytes(&seed);
                    let vk = sk.verifying_key();
                    let id = opolys_crypto::ed25519_public_key_to_object_id(vk.as_bytes());
                    tracing::info!(miner_id = %id.to_hex(), "Loaded miner/validator identity from key file");
                    (id, Some(sk))
                }
                Ok(bytes) => {
                    tracing::error!("Key file must be exactly 32 bytes, got {}", bytes.len());
                    (ObjectId(Hash::zero()), None)
                }
                Err(e) => {
                    tracing::warn!("Failed to read key file {:?}: {}. Using zero miner_id.", key_path, e);
                    (ObjectId(Hash::zero()), None)
                }
            }
        } else {
            (ObjectId(Hash::zero()), None)
        };

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
                        let mut accounts = AccountStore::new();
                        let validators = ValidatorSet::new();
                        // Credit genesis accounts with their initial balances
                        let genesis_issued = opolys_consensus::genesis::apply_genesis_accounts(
                            &genesis_config, &mut accounts,
                        );
                        // Track genesis issuance in chain state
                        let mut chain = chain;
                        chain.total_issued = chain.total_issued.saturating_add(genesis_issued);
                        (chain, accounts, validators, Some(store))
                    }
                    Err(e) => {
                        tracing::error!("Failed to load chain state: {}, starting fresh", e);
                        let chain = ChainState::new(&genesis_config);
                        let mut accounts = AccountStore::new();
                        let validators = ValidatorSet::new();
                        let genesis_issued = opolys_consensus::genesis::apply_genesis_accounts(
                            &genesis_config, &mut accounts,
                        );
                        let mut chain = chain;
                        chain.total_issued = chain.total_issued.saturating_add(genesis_issued);
                        (chain, accounts, validators, Some(store))
                    }
                }
            }
            Err(e) => {
                tracing::warn!("Could not open database at {:?}: {}, running without persistence", data_path, e);
                let chain_state = ChainState::new(&genesis_config);
                let mut accounts = AccountStore::new();
                let validators = ValidatorSet::new();
                let genesis_issued = opolys_consensus::genesis::apply_genesis_accounts(
                    &genesis_config, &mut accounts,
                );
                let mut chain_state = chain_state;
                chain_state.total_issued = chain_state.total_issued.saturating_add(genesis_issued);
                (chain_state, accounts, validators, None)
            }
        };

        OpolysNode {
            chain: Arc::new(RwLock::new(chain_state)),
            accounts: Arc::new(RwLock::new(accounts)),
            mempool: Arc::new(RwLock::new(Mempool::new())),
            validators: Arc::new(RwLock::new(validators)),
            store,
            config: config.clone(),
            pow_context: Arc::new(RwLock::new(PowContext::new())),
            miner_id: miner_id.clone(),
            signing_key,
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
            producer: self.miner_id.clone(),
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

    /// Produce a PoS block as a validator.
    ///
    /// When `--validate` is enabled and this node's `miner_id` is the
    /// **selected** block producer (determined by weighted random sampling
    /// seeded from the previous block hash), this method builds and signs a
    /// block. The block contains no PoW proof; instead, the validator signs
    /// the block hash with their ed25519 key, and the signature is stored in
    /// `validator_signature`.
    ///
    /// The producer is selected via `ValidatorSet::select_block_producer()`,
    /// which uses the previous block hash as entropy for deterministic,
    /// verifiable selection. Any node can verify that the producer was
    /// legitimately chosen by re-running the selection with the same seed.
    ///
    /// Returns `Some(Block)` if this node is the selected producer, or `None`
    /// if another validator was selected or no signing key is available.
    pub async fn produce_pos_block(&self) -> Option<Block> {
        let signing_key = self.signing_key.as_ref()?;
        let chain = self.chain.read().await;
        let validators = self.validators.read().await;
        let mempool = self.mempool.read().await;

        // Derive deterministic producer selection seed from the previous block hash.
        // This ensures every node computes the same producer for the same height.
        let seed = u64::from_be_bytes(
            chain.latest_block_hash.0[0..8].try_into().unwrap_or([0u8; 8])
        );

        // Select the block producer via weighted random sampling
        let producer = validators.select_block_producer(
            chain.block_timestamps.last().copied().unwrap_or(0),
            seed,
        )?;

        // Only produce if this node is the selected producer
        if producer.object_id != self.miner_id {
            tracing::debug!(
                expected_producer = %producer.object_id.to_hex(),
                our_id = %self.miner_id.to_hex(),
                "Not selected as block producer, skipping"
            );
            return None;
        }

        // Build block from mempool transactions
        let transactions: Vec<Transaction> = mempool.get_ordered_transactions()
            .into_iter()
            .take(MAX_TRANSACTIONS_PER_BLOCK)
            .cloned()
            .collect();

        let transaction_root = compute_transaction_root(&transactions);
        let bonded_stake = validators.total_bonded_stake();

        let diff_target = compute_next_difficulty(
            chain.current_difficulty,
            chain.current_height,
            &chain.block_timestamps,
            chain.total_issued,
            bonded_stake,
        );
        let difficulty = diff_target.effective_difficulty();

        // Build the block header (no PoW proof)
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
            producer: self.miner_id.clone(),
            pow_proof: None,
            validator_signature: None,
        };

        // Compute the block hash and sign it with the validator's ed25519 key
        let block_hash = compute_block_hash(&header);
        let signature: ed25519_dalek::Signature = signing_key.sign(block_hash.0.as_ref());
        let validator_signature = signature.to_bytes().to_vec();

        let block = Block {
            header: BlockHeader {
                validator_signature: Some(validator_signature),
                ..header
            },
            transactions,
        };

        tracing::info!(
            height = block.header.height,
            producer = %self.miner_id.to_hex(),
            "Produced PoS block"
        );

        Some(block)
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

        // Verify PoS validator signature if present
        if block.header.validator_signature.is_some() && !block.header.producer.0.is_zero() {
            if let Some(ref sig_bytes) = block.header.validator_signature {
                if sig_bytes.len() == 64 {
                    let block_hash = compute_block_hash(&block.header);
                    // Look up the producer's public key from their on-chain account
                    if let Some(account) = accounts.get_account(&block.header.producer) {
                        if let Some(ref pk_bytes) = account.public_key {
                            if pk_bytes.len() == 32 {
                                let mut sig_array = [0u8; 64];
                                sig_array.copy_from_slice(sig_bytes);
                                let mut pk_array = [0u8; 32];
                                pk_array.copy_from_slice(pk_bytes);
                                if let Ok(verifying_key) = ed25519_dalek::VerifyingKey::from_bytes(&pk_array) {
                                    let signature = ed25519_dalek::Signature::from_bytes(&sig_array);
                                    if verifying_key.verify(block_hash.0.as_ref(), &signature).is_err() {
                                        return Err("PoS block validator signature verification failed".to_string());
                                    }
                                }
                            }
                        }
                    }
// If the producer has no stored public key yet, we accept the block
                    // (first-time producers need their first transaction to register the key)
                }
            }
        }

        // Verify PoS block producer was legitimately selected
        if block.header.validator_signature.is_some() && !block.header.producer.0.is_zero() {
            let seed = u64::from_be_bytes(
                chain.latest_block_hash.0[0..8].try_into().unwrap_or([0u8; 8])
            );
            let timestamp = chain.block_timestamps.last().copied().unwrap_or(0);
            if let Some(expected_producer) = validators.select_block_producer(timestamp, seed) {
                if expected_producer.object_id != block.header.producer {
                    return Err(format!(
                        "PoS block producer mismatch: expected {}, got {}",
                        expected_producer.object_id.to_hex(),
                        block.header.producer.to_hex()
                    ));
                }
            }
        }

        // Detect double-signing: if a validator signed a different block at
        // the same height, slash them. This is the only slashing condition.
        if block.header.validator_signature.is_some() && !block.header.producer.0.is_zero() {
            let block_hash = compute_block_hash(&block.header);
            let key = (block.header.height, block.header.producer.to_hex());
            if let Some(previous_hash) = chain.producer_signatures.get(&key) {
                if *previous_hash != block_hash {
                    // Double-sign detected! Slash the validator.
                    tracing::warn!(
                        producer = %block.header.producer.to_hex(),
                        height = block.header.height,
                        "Double-sign detected! Slashing validator"
                    );
                    if let Ok(slashed_amount) = validators.slash(&block.header.producer) {
                        chain.total_burned = chain.total_burned.saturating_add(slashed_amount);
                        tracing::info!(
                            producer = %block.header.producer.to_hex(),
                            amount = slashed_amount,
                            "Validator slashed for double-signing"
                        );
                    }
                }
            } else {
                chain.producer_signatures.insert(key, block_hash);
            }
        }

        // Compute vein yield from the EVO-OMAP PoW hash
        let pow_hash_value = pow::compute_pow_hash_value(&block.header).unwrap_or(0u64);

        // Block reward uses vein yield: BASE_REWARD / difficulty * vein_yield
        let block_reward = emission::compute_block_reward(block.header.difficulty, pow_hash_value);

        // Credit the block reward to the producer's account.
        // The producer is the miner (PoW) or validator (PoS) identified by
        // block.header.producer. If the producer account doesn't exist yet,
        // it is auto-created with zero balance before crediting.
        let producer = &block.header.producer;
        if !producer.0.is_zero() && block_reward > 0 {
            if accounts.get_account(producer).is_none() {
                accounts.create_account(producer.clone()).ok();
            }
            accounts.credit(producer, block_reward).ok();
        }

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

        // Process matured unbonding entries — return stake to accounts
        for (account, amount) in validators.process_matured_unbonds(chain.current_height) {
            if accounts.get_account(&account).is_none() {
                accounts.create_account(account.clone()).ok();
            }
            accounts.credit(&account, amount).ok();
            tracing::debug!(
                account = %account.to_hex(),
                amount,
                "Matured unbonding entry credited"
            );
        }

        // Activate validators that have been bonding for at least one epoch
        let activated = validators.activate_matured_validators(chain.current_height);
        if !activated.is_empty() {
            tracing::info!(
                count = activated.len(),
                "Validators activated at epoch boundary"
            );
        }

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
            validate: false,
            key_file: None,
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