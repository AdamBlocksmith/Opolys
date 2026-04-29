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
use opolys_networking::PeerId;
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

    /// Bootstrap peer addresses for initial network discovery.
    /// Accepts multiple addresses separated by commas, or repeated --bootstrap flags.
    /// These are added on top of the hardcoded default peers for the selected network.
    #[arg(long, value_delimiter = ',')]
    pub bootstrap: Vec<String>,

    /// Skip all hardcoded and DNS-resolved bootstrap peers.
    /// User-provided --bootstrap addresses are still dialed.
    /// Useful for isolated local testnets where you control all peers manually.
    #[arg(long)]
    pub no_bootstrap: bool,

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

    /// Run in testnet mode with pre-funded genesis accounts.
    ///
    /// Creates 3 genesis accounts each funded with 10,000 OPL for testing.
    /// These accounts have deterministic ObjectIds derived from well-known
    /// testnet keys. Do NOT use testnet mode for production.
    #[arg(long)]
    pub testnet: bool,

    /// RPC server listen address (default: 127.0.0.1 — localhost only).
    ///
    /// By default the RPC server only accepts local connections.
    /// To expose the RPC to external clients pass --rpc-listen-addr 0.0.0.0.
    /// WARNING: exposing the RPC publicly without --rpc-api-key is a security risk.
    #[arg(long, default_value = "127.0.0.1")]
    pub rpc_listen_addr: String,

    /// Optional API key for write and mining RPC methods.
    ///
    /// If set, opl_sendTransaction, opl_getMiningJob, and opl_submitSolution
    /// require Authorization: Bearer <key> or X-Api-Key: <key> header.
    /// All read methods (balance, blocks, chain info, etc.) remain public.
    #[arg(long)]
    pub rpc_api_key: Option<String>,
}

/// Configuration for an Opolys node, derived from CLI arguments or defaults.
#[derive(Debug, Clone)]
pub struct NodeConfig {
    pub listen_port: u16,
    pub rpc_port: u16,
    pub data_dir: String,
    pub bootstrap_peers: Vec<String>,
    /// Skip hardcoded and DNS bootstrap peers; only dial user-provided peers.
    pub no_bootstrap: bool,
    pub log_level: String,
    pub mine: bool,
    pub no_rpc: bool,
    pub validate: bool,
    /// Path to the miner/validator key file (32-byte ed25519 seed).
    /// When provided, the node can sign PoS blocks and receive block rewards.
    pub key_file: Option<String>,
    /// Run in testnet mode with pre-funded genesis accounts.
    /// Creates 3 accounts each with 10,000 OPL. Do NOT use in production.
    pub testnet: bool,
    /// IP address the RPC server listens on. Default: "127.0.0.1".
    /// Set to "0.0.0.0" to expose publicly (use with --rpc-api-key).
    pub rpc_listen_addr: String,
    /// Optional API key for write and mining RPC endpoints.
    pub rpc_api_key: Option<String>,
}

/// Build the testnet genesis config with pre-funded accounts.
///
/// These are deterministic testnet keys — NOT for production use.
/// Each account starts with 10,000 OPL (10,000,000,000 Flakes).
fn testnet_genesis_config() -> opolys_consensus::GenesisConfig {
    use opolys_core::FLAKES_PER_OPL;
    let mut config = opolys_consensus::GenesisConfig::default();
    config.initial_difficulty = 4; // testnet: faster blocks for testing
    // 10,000 OPL per testnet account
    let testnet_funding = 10_000 * FLAKES_PER_OPL;
    config.genesis_accounts = vec![
        // Testnet Account 0 — see testnet-data/testnet-keys.txt for seed
        (
            ObjectId::from_hex("12865e52536fc1d6e63e1c5430f01134efd540514b4d66df76a990dd7875dc16").unwrap(),
            testnet_funding,
            hex::decode("7e6db137e7e59a3f96ae682b5c7292f9ecc0529f8c55c728a631456190a97a66").unwrap(),
        ),
        // Testnet Account 1 — see testnet-data/testnet-keys.txt for seed
        (
            ObjectId::from_hex("558af9966416c04a2b2aff355f386aeb5d356b100861992069b8592a0021dc8b").unwrap(),
            testnet_funding,
            hex::decode("8aef2fc5caa3343aac5000072d2a6fe837746f912c6c26b1071b39c2b83a35c4").unwrap(),
        ),
        // Testnet Account 2 — see testnet-data/testnet-keys.txt for seed
        (
            ObjectId::from_hex("e024a035a42f9858bb498f0a64c28d9702783265fb7f6cc484b7f986d48eef9d").unwrap(),
            testnet_funding,
            hex::decode("cd606e8a8f63b78c1a8fb2f063bd9a9db8699dc44a6f0447ba2505276efca57a").unwrap(),
        ),
    ];
    config
}

impl Default for NodeConfig {
    fn default() -> Self {
        NodeConfig {
            listen_port: DEFAULT_LISTEN_PORT,
            rpc_port: DEFAULT_LISTEN_PORT + 1,
            data_dir: "./data".to_string(),
            bootstrap_peers: vec![],
            no_bootstrap: false,
            log_level: "info".to_string(),
            mine: false,
            no_rpc: false,
            validate: false,
            key_file: None,
            testnet: false,
            rpc_listen_addr: "127.0.0.1".to_string(),
            rpc_api_key: None,
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
    /// Double-sign detection: tracks (block_hash, validator_signature) per (height, producer).
    /// When a second different hash is seen for the same key, evidence is queued.
    pub producer_signatures: HashMap<(u64, String), (Hash, Vec<u8>)>,
    /// The ceremony-derived block reward for this chain in Flakes.
    /// Mainnet: read from the genesis ceremony attestation.
    /// Testnet/dev: the BASE_REWARD constant (312 OPL).
    pub base_reward: FlakeAmount,
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
            base_reward: genesis_config.base_reward,
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
            producer_signatures: p.producer_signatures.iter().map(|(h, prod, hash, sig)| {
                ((*h, prod.clone()), (hash.clone(), sig.clone()))
            }).collect(),
            // Migration: nodes upgraded from pre-ceremony builds get the constant default
            base_reward: if p.base_reward > 0 { p.base_reward } else { BASE_REWARD },
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
            base_reward: self.base_reward,
            producer_signatures: self.producer_signatures.iter().map(|((h, prod), (hash, sig))| {
                (*h, prod.clone(), hash.clone(), sig.clone())
            }).collect(),
        }
    }

    /// Circulating supply = total_issued - total_burned.
    pub fn circulating_supply(&self) -> FlakeAmount {
        self.total_issued.saturating_sub(self.total_burned)
    }

    /// Stake coverage = bonded_stake / total_issued.
    ///
    /// Requires the actual bonded stake from the validator set — this cannot
    /// be computed from chain state alone since bonded stake lives in
    /// ValidatorSet, not ChainState. Passing total_issued for both parameters
    /// would always return 1.0, which is the critical bug this method now
    /// avoids by requiring the caller to supply bonded_stake.
    pub fn stake_coverage(&self, bonded_stake: FlakeAmount) -> f64 {
        emission::compute_stake_coverage(
            bonded_stake,
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
    /// Double-sign evidence detected locally, pending inclusion in the next mined block.
    /// Drained into `Block.slash_evidence` by mine_block() and produce_pos_block().
    pub pending_slash_evidence: Arc<RwLock<Vec<DoubleSignEvidence>>>,
    /// Peers that have announced an active validator identity via the identify protocol.
    /// Keyed by libp2p PeerId; value is their on-chain ObjectId (used for look-ups).
    pub validator_peers: Arc<RwLock<HashMap<PeerId, ObjectId>>>,
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

        let genesis_config = if config.testnet {
            tracing::warn!("TESTNET MODE: Pre-funded genesis accounts enabled. DO NOT use in production.");
            testnet_genesis_config()
        } else {
            opolys_consensus::GenesisConfig::default()
        };

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
            pending_slash_evidence: Arc::new(RwLock::new(Vec::new())),
            validator_peers: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Return true if the given peer has announced itself as an active on-chain validator.
    pub async fn is_validator_peer(&self, peer_id: &PeerId) -> bool {
        let validator_peers = self.validator_peers.read().await;
        if let Some(object_id) = validator_peers.get(peer_id) {
            let validators = self.validators.read().await;
            if let Some(v) = validators.get_validator(object_id) {
                return v.status == ValidatorStatus::Active;
            }
        }
        false
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
            .take(MAX_TRANSACTIONS_PER_BLOCK)
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

        // Drain pending double-sign evidence before mining starts
        let pending_evidence: Vec<DoubleSignEvidence> = {
            let mut pending = self.pending_slash_evidence.write().await;
            std::mem::take(&mut *pending)
        };

        drop(chain);
        drop(accounts);
        drop(validators);
        drop(mempool);

        let mut ctx = self.pow_context.write().await;
        let mut block = ctx.mine_parallel(header, difficulty, max_attempts, 0)?;
        block.slash_evidence = pending_evidence;
        Some(block)
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

        // Drain pending double-sign evidence into this block
        let pending_evidence: Vec<DoubleSignEvidence> = {
            let mut pending = self.pending_slash_evidence.write().await;
            std::mem::take(&mut *pending)
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
            slash_evidence: pending_evidence,
            genesis_ceremony: None,
        };

        tracing::info!(
            height = block.header.height,
            producer = %self.miner_id.to_hex(),
            "Produced PoS block"
        );

        Some(block)
    }

    /// Verify that a `DoubleSignEvidence` item is cryptographically valid.
    ///
    /// Checks: hashes differ, pubkey length is 32, pubkey Blake3-hashes to `producer`,
    /// and both ed25519 signatures are valid over their respective hashes.
    fn verify_slash_evidence(evidence: &DoubleSignEvidence) -> bool {
        if evidence.hash_a == evidence.hash_b {
            return false;
        }
        if evidence.producer_pubkey.len() != 32 {
            return false;
        }
        let pk_arr: [u8; 32] = match evidence.producer_pubkey.as_slice().try_into() {
            Ok(a) => a,
            Err(_) => return false,
        };
        let expected_id = opolys_crypto::ed25519_public_key_to_object_id(&pk_arr);
        if expected_id != evidence.producer {
            return false;
        }
        opolys_crypto::verify_ed25519(&evidence.producer_pubkey, evidence.hash_a.0.as_ref(), &evidence.signature_a)
            && opolys_crypto::verify_ed25519(&evidence.producer_pubkey, evidence.hash_b.0.as_ref(), &evidence.signature_b)
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

        // STATE ROOT CONVENTION (Ethereum-style parent-state-root):
        // block.header.state_root = state root computed at end of block N-1
        // This is the pre-execution state of block N.
        // We verify it matches our local chain.state_root (also post-block N-1)
        // BEFORE executing any transactions in block N.
        // The new state root computed at the END of apply_block() will go
        // into block N+1's header — not this block's header.
        // Do not move this check after transaction execution.
        if block.header.state_root != chain.state_root {
            return Err(format!(
                "State root mismatch at height {}: expected {}, got {}",
                block.header.height,
                chain.state_root.to_hex(),
                block.header.state_root.to_hex()
            ));
        }

        // Verify PoS validator signature if present
        if block.header.validator_signature.is_some() && !block.header.producer.0.is_zero() {
            if let Some(ref sig_bytes) = block.header.validator_signature {
                if sig_bytes.len() != 64 {
                    return Err("PoS block validator signature must be 64 bytes".to_string());
                }
                let block_hash = compute_block_hash(&block.header);
                let (pk_array, sig_array) = {
                    let account = accounts.get_account(&block.header.producer)
                        .ok_or_else(|| "PoS block producer account not found".to_string())?;
                    let pk_bytes = account.public_key.as_ref()
                        .ok_or_else(|| "PoS block producer public key not registered".to_string())?;
                    if pk_bytes.len() != 32 {
                        return Err("PoS block producer public key must be 32 bytes".to_string());
                    }
                    let mut pk_array = [0u8; 32];
                    pk_array.copy_from_slice(pk_bytes);
                    let mut sig_array = [0u8; 64];
                    sig_array.copy_from_slice(sig_bytes);
                    (pk_array, sig_array)
                };
                let verifying_key = ed25519_dalek::VerifyingKey::from_bytes(&pk_array)
                    .map_err(|_| "PoS block producer public key is invalid".to_string())?;
                let signature = ed25519_dalek::Signature::from_bytes(&sig_array);
                verifying_key.verify(block_hash.0.as_ref(), &signature)
                    .map_err(|_| "PoS block validator signature verification failed".to_string())?;
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

        // Process double-sign evidence embedded in this block.
        // Each item is verified independently; duplicates within the block are skipped.
        let mut processed_evidence_keys: std::collections::HashSet<(String, u64)> = std::collections::HashSet::new();
        for evidence in &block.slash_evidence {
            let dedup_key = (evidence.producer.to_hex(), evidence.height);
            if processed_evidence_keys.contains(&dedup_key) {
                continue;
            }
            if Self::verify_slash_evidence(evidence) {
                processed_evidence_keys.insert(dedup_key);
                match validators.graduated_slash(&evidence.producer, block.header.height) {
                    Ok(burned) if burned > 0 => {
                        chain.total_burned = chain.total_burned.saturating_add(burned);
                        tracing::info!(
                            producer = %evidence.producer.to_hex(),
                            burned,
                            offense = ?validators.get_validator(&evidence.producer).map(|v| v.slash_offense_count),
                            "Graduated slash applied from block evidence"
                        );
                    }
                    Ok(_) => {} // already permanently slashed, no-op
                    Err(e) => tracing::warn!(error = %e, "Evidence processing failed"),
                }
            } else {
                tracing::warn!(
                    producer = %evidence.producer.to_hex(),
                    height = evidence.height,
                    "Invalid slash evidence in block — rejected"
                );
            }
        }

        // Detect double-signing locally: if a validator signed a different block at
        // the same height, build evidence for inclusion in the next mined block.
        let mut new_evidence: Vec<DoubleSignEvidence> = Vec::new();
        if block.header.validator_signature.is_some() && !block.header.producer.0.is_zero() {
            let block_hash = compute_block_hash(&block.header);
            let sig_bytes = block.header.validator_signature.as_ref().unwrap().clone();
            let key = (block.header.height, block.header.producer.to_hex());
            if let Some((prev_hash, prev_sig)) = chain.producer_signatures.get(&key) {
                if *prev_hash != block_hash {
                    tracing::warn!(
                        producer = %block.header.producer.to_hex(),
                        height = block.header.height,
                        "Double-sign detected locally — queuing evidence for next block"
                    );
                    let pubkey = accounts.get_account(&block.header.producer)
                        .and_then(|a| a.public_key.clone())
                        .unwrap_or_default();
                    new_evidence.push(DoubleSignEvidence {
                        producer: block.header.producer.clone(),
                        producer_pubkey: pubkey,
                        height: block.header.height,
                        hash_a: prev_hash.clone(),
                        signature_a: prev_sig.clone(),
                        hash_b: block_hash,
                        signature_b: sig_bytes,
                    });
                }
            } else {
                chain.producer_signatures.insert(key, (block_hash, sig_bytes));
            }
        }

        // Genesis block (height 0) issues zero reward.
        // Supply starts at exactly zero.
        // First OPL enters circulation at block 1 when real mining begins.
        // Matches gold analogy — nobody found gold until someone dug.
        let block_reward = if block.header.height == 0 {
            0
        } else {
            let pow_hash_value = if block.header.pow_proof.is_some() {
                // PoW block: use actual hash value for vein yield calculation
                // Lucky hashes (small hash_int) earn higher yield
                pow::compute_pow_hash_value(&block.header).unwrap_or(0u64)
            } else {
                // PoS block: validators earn flat base reward with no luck component
                // Deliberate design: vaults earn steady fees, miners earn variable ore
                // hash_int = 0 triggers the 1.0x floor in compute_vein_yield()
                // This is intentional — not a missing feature
                0u64
            };
            emission::compute_block_reward(chain.base_reward, block.header.difficulty, pow_hash_value)
        };

        // Split the block reward between miners and validators based on stake coverage.
        // pow_share goes to the block producer (miner or selected validator).
        // pos_share is distributed among all active validators proportional to weight.
        // coverage_milli = (bonded_stake × 1000) / total_issued, avoiding floating point.
        let coverage_milli: u64 = if chain.total_issued > 0 {
            ((bonded_stake as u128 * 1000) / chain.total_issued as u128).min(1000) as u64
        } else {
            0
        };
        // pow_share_amount = block_reward × (1000 - coverage_milli) / 1000
        // pos_share_amount = block_reward - pow_share_amount (avoids rounding drift)
        let pow_share_amount = ((block_reward as u128 * (1000 - coverage_milli) as u128) / 1000) as FlakeAmount;
        let pos_share_amount = block_reward.saturating_sub(pow_share_amount);

        // Credit the PoW share to the block producer (miner or selected validator).
        // The producer is identified by block.header.producer.
        let producer = &block.header.producer;
        if !producer.0.is_zero() && pow_share_amount > 0 {
            if accounts.get_account(producer).is_none() {
                accounts.create_account(producer.clone()).ok();
            }
            accounts.credit(producer, pow_share_amount).ok();
        }

        // Distribute the PoS share among active validators proportional to weight.
        // Each validator's share = pos_share_amount × (their_weight / total_weight).
        if pos_share_amount > 0 {
            let current_timestamp = chain.block_timestamps.last().copied().unwrap_or(0);
            let total_weight = validators.total_weight(current_timestamp);
            if total_weight > 0 {
                for v in validators.active_validators() {
                    let v_weight = v.weight(current_timestamp);
                    let v_share = ((pos_share_amount as u128 * v_weight as u128) / total_weight as u128) as FlakeAmount;
                    if v_share > 0 {
                        if accounts.get_account(&v.object_id).is_none() {
                            accounts.create_account(v.object_id.clone()).ok();
                        }
                        accounts.credit(&v.object_id, v_share).ok();
                    }
                }
            }
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
        let expected_chain_id = if self.config.testnet { TESTNET_CHAIN_ID } else { MAINNET_CHAIN_ID };
        let mut total_fees_burned: FlakeAmount = 0;
        for tx in &block.transactions {
            let result = TransactionDispatcher::apply_transaction(
                tx,
                &mut accounts,
                &mut validators,
                block.header.height,
                block.header.timestamp,
                expected_chain_id,
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

        // Update consensus phase based on stake coverage.
        // Any bonded stake shifts to PoS — the smooth transition model means
        // there's no threshold, just a gradual shift as coverage increases.
        let coverage = chain.stake_coverage(bonded_stake);
        if bonded_stake > 0 && coverage > 0.0 {
            chain.phase = ConsensusPhase::ProofOfStake;
        } else {
            chain.phase = ConsensusPhase::ProofOfWork;
        }

        // This state root goes into the NEXT block's header (block N+1).
        // It reflects all state changes from this block:
        // rewards, transactions, unbonds, validator activations.
        let mut account_hasher = opolys_crypto::Blake3Hasher::new();
        account_hasher.update(accounts.compute_state_root().as_bytes());
        account_hasher.update(validators.compute_state_root().as_bytes());
        account_hasher.update(&chain.total_issued.to_be_bytes());
        account_hasher.update(&chain.total_burned.to_be_bytes());
        account_hasher.update(&chain.current_height.to_be_bytes());
        chain.state_root = account_hasher.finalize();

        // Persist state to disk
        if let Some(ref store) = self.store {
            if let Err(e) = Self::persist_state(store, &chain, &accounts, &validators, block) {
                tracing::error!("Failed to persist state: {}", e);
            }
        }

        // Release all write locks before accessing pending_slash_evidence
        drop(mempool);
        drop(validators);
        drop(accounts);
        drop(chain);

        // Queue newly detected evidence for inclusion in the next mined block
        if !new_evidence.is_empty() {
            self.pending_slash_evidence.write().await.extend(new_evidence);
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
            no_bootstrap: true,
            log_level: "warn".to_string(),
            mine: true,
            no_rpc: true,
            validate: false,
            key_file: None,
            testnet: false,
            rpc_listen_addr: "127.0.0.1".to_string(),
            rpc_api_key: None,
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