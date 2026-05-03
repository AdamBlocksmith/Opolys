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
//! Refiners earn from block rewards only. Only double-signing gets slashed. There
//! is no governance, no schedules, and no fixed percentages.
//!
//! Hashing: Blake3-256 (32 bytes) everywhere. Signatures: ed25519.
//! Key derivation: BIP-39 24-word mnemonics, SLIP-0010 ed25519.

use clap::Parser;
use ed25519_dalek::{Signer, Verifier};
use opolys_consensus::block::{compute_block_hash, compute_transaction_root};
use opolys_consensus::difficulty::compute_next_difficulty;
use opolys_consensus::emission::compute_suggested_fee;
use opolys_consensus::pow;
use opolys_consensus::{
    account::AccountStore,
    emission,
    genesis::{GenesisAttestation as ConsensusGenesisAttestation, GenesisConfig},
    mempool::Mempool,
    pow::PowContext,
    refiner::RefinerSet,
};
use opolys_core::*;
use opolys_crypto::{
    DOMAIN_STATE_ROOT, block_attestation_signing_payload, refiner_block_signing_payload,
};
use opolys_execution::TransactionDispatcher;
use opolys_networking::PeerId;
use opolys_storage::BlockchainStore;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

/// A record of a banned peer, persisted across restarts.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct BanRecord {
    /// Short description of why the peer was banned.
    pub reason: String,
    /// Unix timestamp when the ban was issued.
    pub banned_at: u64,
    /// Number of times this peer has been banned (escalates duration).
    pub ban_count: u32,
    /// True for severe violations (fake PoW, invalid signature) — never expires.
    pub permanent: bool,
}

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
    /// Useful for isolated local networks where you control all peers manually.
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

    /// Enable refiner block production (default: disabled).
    ///
    /// When enabled, the node will produce refiner blocks when it is an active
    /// refiner with bonded stake. Requires a wallet key to sign blocks.
    /// This flag is separate from --mine (both can be active simultaneously).
    #[arg(long)]
    pub refine: bool,

    /// Path to the miner/refiner key file (32-byte ed25519 seed).
    ///
    /// The ObjectId (Blake3 hash of the public key) derived from this key
    /// is used as the block producer identity. If not provided, the miner_id
    /// defaults to zero (rewards are not credited to any account).
    /// For production use, generate a key with `opl keygen` and provide the path.
    #[arg(long)]
    pub key_file: Option<String>,

    /// Path to the genesis ceremony attestation JSON file.
    ///
    /// Required for mainnet operation. Generate it with `genesis-ceremony`.
    /// The node verifies the ceremony master hash, operator signature, source
    /// counts, manual evidence, and base-reward derivation before startup.
    #[arg(long)]
    pub genesis_params: Option<String>,

    /// RPC server listen address (default: 127.0.0.1 — localhost only).
    ///
    /// By default the RPC server only accepts local connections.
    /// To expose the RPC to external clients pass --rpc-listen-addr 0.0.0.0.
    /// WARNING: exposing the RPC publicly without authentication is a security risk.
    #[arg(long, default_value = "127.0.0.1")]
    pub rpc_listen_addr: String,

    /// API key for write and mining RPC methods.
    ///
    /// If set, opl_sendTransaction, opl_getMiningJob, and opl_submitSolution
    /// require Authorization: Bearer <key> or X-Api-Key: <key> header.
    /// All read methods (balance, blocks, chain info, etc.) remain public.
    /// If omitted, the node generates a random key at startup.
    #[arg(long, conflicts_with = "no_rpc_auth")]
    pub rpc_api_key: Option<String>,

    /// Disable API-key auth for write and mining RPC methods.
    ///
    /// Mainnet operators should avoid this unless the RPC server is fully
    /// isolated behind another authenticated service.
    #[arg(long)]
    pub no_rpc_auth: bool,
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
    pub refine: bool,
    /// Path to the miner/refiner key file (32-byte ed25519 seed).
    /// When provided, the node can sign refiner blocks and receive block rewards.
    pub key_file: Option<String>,
    /// IP address the RPC server listens on. Default: "127.0.0.1".
    /// Set to "0.0.0.0" to expose publicly (use with --rpc-api-key).
    pub rpc_listen_addr: String,
    /// Optional API key for write and mining RPC endpoints.
    pub rpc_api_key: Option<String>,
    /// Path to the genesis ceremony JSON. Required on startup.
    pub genesis_params_path: Option<String>,
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
            refine: false,
            key_file: None,
            rpc_listen_addr: "127.0.0.1".to_string(),
            rpc_api_key: None,
            genesis_params_path: None,
        }
    }
}

impl NodeConfig {
    /// Chain id for this node configuration.
    ///
    /// Opolys is mainnet-only. Keeping this explicit makes persisted state
    /// fail closed if a database was created with any other chain id.
    pub fn chain_id(&self) -> u64 {
        MAINNET_CHAIN_ID
    }

    /// Mainnet data directory.
    ///
    /// Persistent node files live below `<data_dir>/mainnet`, making the
    /// on-disk layout explicit for the only supported network.
    pub fn chain_data_dir(&self) -> PathBuf {
        chain_data_dir(&self.data_dir, self.chain_id())
    }
}

/// Return the mainnet storage directory for a base data directory.
pub fn chain_data_dir(data_dir: &str, chain_id: u64) -> PathBuf {
    assert_eq!(chain_id, MAINNET_CHAIN_ID, "Opolys supports mainnet only");
    Path::new(data_dir).join("mainnet")
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
    /// Blake3-256 hash of the genesis block header for this chain.
    pub genesis_hash: Hash,
    /// Blake3-256 hash of the most recent block header.
    pub latest_block_hash: Hash,
    /// Blake3-256 hash of the state root after applying the most recent block.
    pub state_root: Hash,
    /// Suggested fee for the next block, computed via EMA of previous block's fees.
    /// Starts at MIN_FEE (1 Flake) and adjusts based on network demand.
    pub suggested_fee: FlakeAmount,
    /// Double-sign detection: tracks (block_hash, refiner_signature) per (height, producer).
    /// When a second different hash is seen for the same key, evidence is queued.
    pub producer_signatures: HashMap<(u64, String), (Hash, Vec<u8>)>,
    /// The ceremony-derived block reward for this chain in Flakes.
    /// Mainnet: read from the genesis ceremony attestation.
    /// Pre-ceremony development builds fall back to the BASE_REWARD constant (332 OPL).
    pub base_reward: FlakeAmount,
    /// Height of the most recently finalized refiner-produced block.
    /// Advances only when later on-chain refiner-block attestations reach
    /// `FINALITY_CONFIDENCE_MILLI` of active refiner weight.
    pub finalized_height: u64,
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
            genesis_hash: genesis_hash.clone(),
            latest_block_hash: genesis_hash,
            state_root: genesis.header.state_root.clone(),
            suggested_fee: MIN_FEE,
            producer_signatures: HashMap::new(),
            base_reward: genesis_config.base_reward,
            finalized_height: 0,
        }
    }

    /// Create chain state from persisted data (loaded from RocksDB).
    pub fn from_persisted(p: &opolys_storage::PersistedChainState) -> Self {
        ChainState {
            current_height: p.current_height,
            current_difficulty: p.current_difficulty,
            total_issued: p.total_issued,
            total_burned: p.total_burned,
            block_timestamps: p.block_timestamps.clone(),
            genesis_hash: Hash::from_bytes(p.genesis_hash),
            latest_block_hash: Hash::from_bytes(p.latest_block_hash),
            state_root: Hash::from_bytes(p.state_root),
            suggested_fee: p.suggested_fee,
            producer_signatures: p
                .producer_signatures
                .iter()
                .map(|(h, prod, hash, sig)| ((*h, prod.clone()), (hash.clone(), sig.clone())))
                .collect(),
            // Migration: nodes upgraded from pre-ceremony builds get the constant default
            base_reward: if p.base_reward > 0 {
                p.base_reward
            } else {
                BASE_REWARD
            },
            finalized_height: p.finalized_height,
        }
    }

    /// Convert chain state to the persisted format for storage.
    pub fn to_persisted(&self) -> opolys_storage::PersistedChainState {
        opolys_storage::PersistedChainState {
            chain_id: MAINNET_CHAIN_ID,
            genesis_hash: self.genesis_hash.0,
            current_height: self.current_height,
            current_difficulty: self.current_difficulty,
            total_issued: self.total_issued,
            total_burned: self.total_burned,
            block_timestamps: self.block_timestamps.clone(),
            latest_block_hash: self.latest_block_hash.0,
            state_root: self.state_root.0,
            suggested_fee: self.suggested_fee,
            base_reward: self.base_reward,
            producer_signatures: self
                .producer_signatures
                .iter()
                .map(|((h, prod), (hash, sig))| (*h, prod.clone(), hash.clone(), sig.clone()))
                .collect(),
            finalized_height: self.finalized_height,
        }
    }

    /// Circulating supply = total_issued - total_burned.
    pub fn circulating_supply(&self) -> FlakeAmount {
        self.total_issued.saturating_sub(self.total_burned)
    }

    /// Stake coverage = bonded_stake / total_issued.
    ///
    /// Requires the actual bonded stake from the refiner set — this cannot
    /// be computed from chain state alone since bonded stake lives in
    /// RefinerSet, not ChainState. Passing total_issued for both parameters
    /// would always return 1.0, which is the critical bug this method now
    /// avoids by requiring the caller to supply bonded_stake.
    pub fn stake_coverage(&self, bonded_stake: FlakeAmount) -> f64 {
        emission::compute_stake_coverage(bonded_stake, self.total_issued)
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
    /// Live refiner set (stake, bonding status).
    pub refiners: Arc<RwLock<RefinerSet>>,
    /// Persistent RocksDB storage (None if running without persistence).
    pub store: Option<Arc<BlockchainStore>>,
    /// Node configuration (ports, data directory, etc.).
    pub config: NodeConfig,
    /// EVO-OMAP mining context with dataset cache for efficient mining.
    pow_context: Arc<RwLock<PowContext>>,
    /// The miner's on-chain identity (Blake3 hash of their public key).
    /// For PoW blocks, this identifies who earns the block reward.
    /// For refiner blocks, this must match an active refiner's ObjectId.
    pub miner_id: ObjectId,
    /// The ed25519 signing key for block production. Set when --key-file is provided.
    /// Used by produce_refiner_block() to sign refiner blocks.
    pub signing_key: Option<ed25519_dalek::SigningKey>,
    /// Double-sign evidence collected during mining or refiner block production.
    /// Drained into `Block.slash_evidence` by mine_block() and produce_refiner_block().
    pub pending_slash_evidence: Arc<RwLock<Vec<DoubleSignEvidence>>>,
    /// Valid refiner attestations collected for future block inclusion.
    /// Keyed by (height, refiner ObjectId hex) to deduplicate repeated gossip.
    pub pending_attestations: Arc<RwLock<HashMap<(u64, String), BlockAttestation>>>,
    /// Peers that have announced an active refiner identity via the identify protocol.
    /// Keyed by libp2p PeerId; value is their on-chain ObjectId (used for look-ups).
    pub refiner_peers: Arc<RwLock<HashMap<PeerId, ObjectId>>>,
    /// Persistent ban list keyed by PeerId string. Loaded from data_dir/banned_peers.json
    /// on startup and saved after every new ban.
    pub banned_peers: Arc<RwLock<HashMap<String, BanRecord>>>,
    /// Peers we dialed (outbound connections). Mining waits until this reaches
    /// MIN_OUTBOUND_FOR_MINING to prevent eclipse attacks where all peers are attacker-controlled.
    pub outbound_peers: Arc<RwLock<HashSet<PeerId>>>,
    /// Peers that dialed us (inbound connections).
    pub inbound_peers: Arc<RwLock<HashSet<PeerId>>>,
}

fn load_ban_list_from_disk(data_dir: &str) -> HashMap<String, BanRecord> {
    let path = std::path::Path::new(data_dir).join("banned_peers.json");
    match std::fs::read_to_string(&path) {
        Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
        Err(_) => HashMap::new(),
    }
}

#[derive(Debug, serde::Deserialize)]
struct CeremonySourceResult {
    name: String,
    raw_response_hash: String,
    extracted_value: Option<f64>,
    #[serde(default)]
    value_origin: String,
    #[serde(default)]
    evidence_note: Option<String>,
    #[serde(default)]
    evidence_timestamp_ms: Option<u64>,
}

#[derive(Debug, serde::Deserialize)]
struct CeremonyAttestationFile {
    ceremony_timestamp: u64,
    operator_public_key: String,
    operator_signature: String,
    production_sources: Vec<CeremonySourceResult>,
    price_sources: Vec<CeremonySourceResult>,
    median_production_tonnes: f64,
    median_price_usd_cents: u64,
    blocks_per_year: u64,
    base_reward_flakes: u64,
    derivation_steps: Vec<String>,
    master_hash: String,
}

const CEREMONY_TROY_OZ_PER_TONNE: f64 = 32_150.7;
const MIN_CEREMONY_PRODUCTION_SOURCES: usize = 5;

fn decode_hex_array<const N: usize>(field: &str, value: &str) -> Result<[u8; N], String> {
    let bytes = hex::decode(value).map_err(|e| format!("{} must be hex: {}", field, e))?;
    bytes
        .try_into()
        .map_err(|bytes: Vec<u8>| format!("{} must be {} bytes, got {}", field, N, bytes.len()))
}

fn source_hash(sources: &[CeremonySourceResult], needle: &str) -> [u8; 32] {
    sources
        .iter()
        .find(|source| source.name.to_ascii_lowercase().contains(needle))
        .and_then(|source| decode_hex_array::<32>(&source.name, &source.raw_response_hash).ok())
        .unwrap_or([0u8; 32])
}

fn validate_ceremony_sources(attestation: &CeremonyAttestationFile) -> Result<(), String> {
    let production_values = attestation
        .production_sources
        .iter()
        .filter_map(|source| source.extracted_value)
        .collect::<Vec<_>>();
    if production_values.len() < MIN_CEREMONY_PRODUCTION_SOURCES {
        return Err(format!(
            "Genesis ceremony has only {} production sources; need at least {}",
            production_values.len(),
            MIN_CEREMONY_PRODUCTION_SOURCES
        ));
    }

    if !attestation
        .price_sources
        .iter()
        .any(|source| source.extracted_value.is_some())
    {
        return Err("Genesis ceremony must include at least one price source".to_string());
    }

    for source in attestation
        .production_sources
        .iter()
        .chain(attestation.price_sources.iter())
    {
        if source.value_origin.starts_with("manual") && source.extracted_value.is_some() {
            let has_evidence = source
                .evidence_note
                .as_deref()
                .is_some_and(|note| note.trim().len() >= 12);
            if !has_evidence || source.evidence_timestamp_ms.is_none() {
                return Err(format!(
                    "Genesis ceremony manual source '{}' is missing evidence",
                    source.name
                ));
            }
        }
    }

    Ok(())
}

fn validate_ceremony_reward_derivation(
    attestation: &CeremonyAttestationFile,
) -> Result<(), String> {
    if !attestation.median_production_tonnes.is_finite()
        || attestation.median_production_tonnes <= 0.0
    {
        return Err("Genesis ceremony median_production_tonnes must be positive".to_string());
    }
    if attestation.blocks_per_year == 0 {
        return Err("Genesis ceremony blocks_per_year must be non-zero".to_string());
    }
    if attestation.base_reward_flakes == 0 {
        return Err("Genesis ceremony base_reward_flakes must be non-zero".to_string());
    }

    let annual_oz = attestation.median_production_tonnes * CEREMONY_TROY_OZ_PER_TONNE;
    let expected_base_reward_opl = (annual_oz / attestation.blocks_per_year as f64).floor() as u64;
    let expected_base_reward_flakes = expected_base_reward_opl
        .checked_mul(FLAKES_PER_OPL)
        .ok_or_else(|| "Genesis ceremony base reward derivation overflowed".to_string())?;
    if attestation.base_reward_flakes != expected_base_reward_flakes {
        return Err(format!(
            "Genesis ceremony base_reward_flakes mismatch: attestation {}, computed {}",
            attestation.base_reward_flakes, expected_base_reward_flakes
        ));
    }

    Ok(())
}

fn compute_ceremony_master_hash(attestation_json: &str) -> Result<[u8; 32], String> {
    let mut value: serde_json::Value = serde_json::from_str(attestation_json)
        .map_err(|e| format!("Genesis ceremony JSON parse failed: {}", e))?;
    value["master_hash"] = serde_json::Value::String(String::new());
    value["operator_signature"] = serde_json::Value::String(String::new());
    let canonical = serde_json::to_string(&value)
        .map_err(|e| format!("Genesis ceremony canonicalization failed: {}", e))?;
    Ok(*blake3::hash(canonical.as_bytes()).as_bytes())
}

fn load_genesis_config_from_attestation(path: &str) -> Result<GenesisConfig, String> {
    let contents = fs::read_to_string(path)
        .map_err(|e| format!("Failed to read genesis ceremony file {}: {}", path, e))?;
    let attestation: CeremonyAttestationFile = serde_json::from_str(&contents)
        .map_err(|e| format!("Failed to parse genesis ceremony file {}: {}", path, e))?;

    validate_ceremony_sources(&attestation)?;
    validate_ceremony_reward_derivation(&attestation)?;

    let computed_master_hash = compute_ceremony_master_hash(&contents)?;
    let stated_master_hash = decode_hex_array::<32>("master_hash", &attestation.master_hash)?;
    if computed_master_hash != stated_master_hash {
        return Err("Genesis ceremony master_hash does not match attestation contents".to_string());
    }

    let operator_public_key =
        decode_hex_array::<32>("operator_public_key", &attestation.operator_public_key)?;
    let operator_signature =
        decode_hex_array::<64>("operator_signature", &attestation.operator_signature)?;
    let verifying_key =
        ed25519_dalek::VerifyingKey::from_bytes(&operator_public_key).map_err(|_| {
            "Genesis ceremony operator_public_key is not a valid ed25519 key".to_string()
        })?;
    let signature = ed25519_dalek::Signature::from_bytes(&operator_signature);
    verifying_key
        .verify_strict(&stated_master_hash, &signature)
        .map_err(|_| "Genesis ceremony operator_signature verification failed".to_string())?;

    let production_tonnes_milli = (attestation.median_production_tonnes * 1000.0).round() as u64;
    let derivation_formula = attestation.derivation_steps.join("; ");
    let lbma_hash = source_hash(&attestation.price_sources, "lbma");
    let usgs_hash = source_hash(&attestation.production_sources, "usgs");
    let wgc_hash = source_hash(&attestation.production_sources, "world gold council");

    let ceremony_data = GenesisCeremonyData {
        ceremony_timestamp: attestation.ceremony_timestamp,
        ceremony_master_hash: stated_master_hash,
        operator_public_key,
        operator_signature,
        base_reward_flakes: attestation.base_reward_flakes,
        production_tonnes_milli,
        price_usd_cents: attestation.median_price_usd_cents,
        blocks_per_year: attestation.blocks_per_year,
    };

    Ok(GenesisConfig {
        initial_difficulty: GENESIS_DIFFICULTY,
        protocol_version: NETWORK_PROTOCOL_VERSION.to_string(),
        attestation: ConsensusGenesisAttestation {
            ceremony_timestamp: attestation.ceremony_timestamp,
            gold_spot_price_usd_cents: attestation.median_price_usd_cents,
            annual_production_tonnes: attestation.median_production_tonnes.round() as u64,
            total_above_ground_tonnes: 0,
            lbma_response_hash: lbma_hash,
            usgs_response_hash: usgs_hash,
            wgc_response_hash: wgc_hash,
            derivation_formula,
        },
        genesis_accounts: vec![],
        base_reward: attestation.base_reward_flakes,
        ceremony_data: Some(ceremony_data),
    })
}

fn genesis_config_for_node(config: &NodeConfig) -> GenesisConfig {
    match config.genesis_params_path.as_deref() {
        Some(path) => load_genesis_config_from_attestation(path).unwrap_or_else(|e| {
            panic!("Invalid genesis ceremony file: {}", e);
        }),
        None => {
            tracing::warn!(
                "No genesis ceremony file configured; using default development genesis"
            );
            GenesisConfig::default()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RewardDistribution {
    gross_reward: FlakeAmount,
    mine_assay: FlakeAmount,
    net_reward: FlakeAmount,
    miner_share: FlakeAmount,
    refiner_share: FlakeAmount,
}

fn compute_reward_distribution(
    gross_reward: FlakeAmount,
    base_reward_amount: FlakeAmount,
    coverage_milli: u64,
) -> RewardDistribution {
    if gross_reward == 0 {
        return RewardDistribution {
            gross_reward: 0,
            mine_assay: 0,
            net_reward: 0,
            miner_share: 0,
            refiner_share: 0,
        };
    }

    let mine_assay =
        ((gross_reward as u128 * ANNUAL_ATTRITION_PERMILLE as u128) / 1000) as FlakeAmount;
    let net_reward = gross_reward.saturating_sub(mine_assay);

    let net_base_reward =
        ((base_reward_amount as u128 * net_reward as u128) / gross_reward as u128) as FlakeAmount;
    let net_vein_bonus = net_reward.saturating_sub(net_base_reward);
    let coverage_milli = coverage_milli.min(1000);

    let miner_base_share =
        ((net_base_reward as u128 * (1000 - coverage_milli) as u128) / 1000) as FlakeAmount;
    let refiner_share = net_base_reward.saturating_sub(miner_base_share);
    let miner_share = miner_base_share.saturating_add(net_vein_bonus);

    RewardDistribution {
        gross_reward,
        mine_assay,
        net_reward,
        miner_share,
        refiner_share,
    }
}

impl OpolysNode {
    /// Create a new node, either loading persisted state from disk or
    /// initializing from genesis.
    pub fn new(config: NodeConfig) -> Self {
        // Load the miner/refiner key from the key file (if provided)
        let (miner_id, signing_key) = if let Some(ref key_path) = config.key_file {
            match std::fs::read(key_path) {
                Ok(seed_bytes) if seed_bytes.len() == 32 => {
                    let mut seed = [0u8; 32];
                    seed.copy_from_slice(&seed_bytes);
                    let sk = ed25519_dalek::SigningKey::from_bytes(&seed);
                    let vk = sk.verifying_key();
                    let id = opolys_crypto::ed25519_public_key_to_object_id(vk.as_bytes());
                    tracing::info!(miner_id = %id.to_hex(), "Loaded miner/refiner identity from key file");
                    (id, Some(sk))
                }
                Ok(bytes) => {
                    tracing::error!("Key file must be exactly 32 bytes, got {}", bytes.len());
                    (ObjectId(Hash::zero()), None)
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to read key file {:?}: {}. Using zero miner_id.",
                        key_path,
                        e
                    );
                    (ObjectId(Hash::zero()), None)
                }
            }
        } else {
            (ObjectId(Hash::zero()), None)
        };

        let genesis_config = genesis_config_for_node(&config);
        let expected_genesis_hash = ChainState::new(&genesis_config).genesis_hash;
        let chain_id = config.chain_id();

        // Try to open the database and load existing state
        let data_path = config.chain_data_dir();
        let store_result = BlockchainStore::open(&data_path);

        let (chain_state, accounts, refiners, store) = match store_result {
            Ok(store) => {
                let store = Arc::new(store);
                match store.load_chain_state() {
                    Ok(Some(persisted)) => {
                        if persisted.chain_id != chain_id {
                            panic!(
                                "Refusing to open data directory {:?}: persisted chain_id {} does not match expected mainnet chain_id {}",
                                data_path, persisted.chain_id, chain_id
                            );
                        } else if Hash::from_bytes(persisted.genesis_hash) != expected_genesis_hash
                        {
                            panic!(
                                "Refusing to open data directory {:?}: persisted genesis {} does not match expected genesis {}",
                                data_path,
                                Hash::from_bytes(persisted.genesis_hash).to_hex(),
                                expected_genesis_hash.to_hex()
                            );
                        } else {
                            tracing::info!(
                                height = persisted.current_height,
                                difficulty = persisted.current_difficulty,
                                issued = persisted.total_issued,
                                chain_id = persisted.chain_id,
                                "Loaded persisted chain state from disk"
                            );
                            let chain = ChainState::from_persisted(&persisted);
                            let accs = store.load_accounts().unwrap_or_else(|e| {
                                panic!(
                                    "Refusing to open data directory {:?}: failed to load persisted accounts: {}",
                                    data_path, e
                                );
                            });
                            let vals = store.load_refiners().unwrap_or_else(|e| {
                                panic!(
                                    "Refusing to open data directory {:?}: failed to load persisted refiners: {}",
                                    data_path, e
                                );
                            });
                            (chain, accs, vals, Some(store))
                        }
                    }
                    Ok(None) => {
                        tracing::info!("No persisted state found, initializing from genesis");
                        let chain = ChainState::new(&genesis_config);
                        let mut accounts = AccountStore::new();
                        let refiners = RefinerSet::new();
                        // Credit genesis accounts with their initial balances
                        let genesis_issued = opolys_consensus::genesis::apply_genesis_accounts(
                            &genesis_config,
                            &mut accounts,
                        );
                        // Track genesis issuance in chain state
                        let mut chain = chain;
                        chain.total_issued = chain.total_issued.saturating_add(genesis_issued);
                        (chain, accounts, refiners, Some(store))
                    }
                    Err(e) => {
                        panic!(
                            "Refusing to open data directory {:?}: failed to load persisted chain state: {}",
                            data_path, e
                        );
                    }
                }
            }
            Err(e) => {
                panic!(
                    "Refusing to start without persistence: failed to open database at {:?}: {}",
                    data_path, e
                );
            }
        };

        let banned_peers = load_ban_list_from_disk(&data_path.to_string_lossy());

        OpolysNode {
            chain: Arc::new(RwLock::new(chain_state)),
            accounts: Arc::new(RwLock::new(accounts)),
            mempool: Arc::new(RwLock::new(Mempool::new())),
            refiners: Arc::new(RwLock::new(refiners)),
            store,
            config: config.clone(),
            pow_context: Arc::new(RwLock::new(PowContext::new())),
            miner_id: miner_id.clone(),
            signing_key,
            pending_slash_evidence: Arc::new(RwLock::new(Vec::new())),
            pending_attestations: Arc::new(RwLock::new(HashMap::new())),
            refiner_peers: Arc::new(RwLock::new(HashMap::new())),
            banned_peers: Arc::new(RwLock::new(banned_peers)),
            outbound_peers: Arc::new(RwLock::new(HashSet::new())),
            inbound_peers: Arc::new(RwLock::new(HashSet::new())),
        }
    }

    /// Return true if the given peer has announced itself as an active on-chain refiner.
    pub async fn is_refiner_peer(&self, peer_id: &PeerId) -> bool {
        let refiner_peers = self.refiner_peers.read().await;
        if let Some(object_id) = refiner_peers.get(peer_id) {
            let refiners = self.refiners.read().await;
            if let Some(v) = refiners.get_refiner(object_id) {
                return v.status == RefinerStatus::Active;
            }
        }
        false
    }

    /// Create a signed attestation for a block this node has accepted.
    ///
    /// Only active refiners with a local key sign attestations. Miners and
    /// non-active refiners return None.
    pub async fn create_block_attestation(
        &self,
        height: u64,
        block_hash: &Hash,
    ) -> Option<BlockAttestation> {
        let signing_key = self.signing_key.as_ref()?;
        {
            let refiners = self.refiners.read().await;
            let refiner = refiners.get_refiner(&self.miner_id)?;
            if refiner.status != RefinerStatus::Active {
                return None;
            }
        }

        let store = self.store.as_ref()?;
        let block = store.load_block(height).ok()??;
        if compute_block_hash(&block.header) != *block_hash {
            return None;
        }
        if block.header.refiner_signature.is_none() || block.header.pow_proof.is_some() {
            return None;
        }

        let payload = block_attestation_signing_payload(height, block_hash);
        let signature: ed25519_dalek::Signature = signing_key.sign(&payload);
        Some(BlockAttestation {
            refiner: self.miner_id.clone(),
            refiner_pubkey: signing_key.verifying_key().as_bytes().to_vec(),
            height,
            block_hash: block_hash.clone(),
            signature: signature.to_bytes().to_vec(),
        })
    }

    /// Verify and store a received refiner block attestation.
    ///
    /// Returns Ok(true) when a new attestation was accepted, Ok(false) when it
    /// was valid but already known, and Err for invalid or non-canonical data.
    pub async fn accept_block_attestation(
        &self,
        attestation: BlockAttestation,
    ) -> Result<bool, String> {
        if attestation.refiner_pubkey.len() != 32 {
            return Err("Attestation public key must be 32 bytes".to_string());
        }
        if attestation.signature.len() != 64 {
            return Err("Attestation signature must be 64 bytes".to_string());
        }

        let pk_arr: [u8; 32] = attestation
            .refiner_pubkey
            .as_slice()
            .try_into()
            .map_err(|_| "Attestation public key must be 32 bytes".to_string())?;
        let expected_refiner = opolys_crypto::ed25519_public_key_to_object_id(&pk_arr);
        if expected_refiner != attestation.refiner {
            return Err("Attestation public key does not match refiner ObjectId".to_string());
        }

        {
            let refiners = self.refiners.read().await;
            let refiner = refiners
                .get_refiner(&attestation.refiner)
                .ok_or_else(|| "Attestation refiner is not bonded".to_string())?;
            if refiner.status != RefinerStatus::Active {
                return Err("Attestation refiner is not active".to_string());
            }
        }

        let Some(store) = &self.store else {
            return Err("Cannot verify attestation without storage".to_string());
        };
        let canonical_block = store
            .load_block(attestation.height)
            .map_err(|e| format!("Failed to load attested block: {}", e))?
            .ok_or_else(|| "Attested block is not known locally".to_string())?;
        if canonical_block.header.refiner_signature.is_none()
            || canonical_block.header.pow_proof.is_some()
        {
            return Err("Attestation target must be a refiner-produced block".to_string());
        }
        let canonical_hash = compute_block_hash(&canonical_block.header);
        if canonical_hash != attestation.block_hash {
            return Err("Attestation block hash is not canonical".to_string());
        }

        let payload =
            block_attestation_signing_payload(attestation.height, &attestation.block_hash);
        if !opolys_crypto::verify_ed25519(
            &attestation.refiner_pubkey,
            &payload,
            &attestation.signature,
        ) {
            return Err("Attestation signature verification failed".to_string());
        }

        let current_height = self.chain.read().await.current_height;
        let min_height = current_height.saturating_sub(EPOCH);
        let key = (attestation.height, attestation.refiner.to_hex());
        let mut pending = self.pending_attestations.write().await;
        pending.retain(|(height, _), _| *height >= min_height);
        if pending.contains_key(&key) {
            return Ok(false);
        }
        pending.insert(key, attestation);
        Ok(true)
    }

    /// Drain collected attestations for inclusion in a newly produced block.
    async fn drain_pending_attestations_for_block(
        &self,
        next_height: u64,
    ) -> Vec<BlockAttestation> {
        let min_height = next_height.saturating_sub(EPOCH);
        let mut pending = self.pending_attestations.write().await;
        pending.retain(|(height, _), _| *height >= min_height && *height < next_height);
        let keys: Vec<(u64, String)> = pending
            .keys()
            .take(MAX_ATTESTATIONS_PER_BLOCK)
            .cloned()
            .collect();
        let mut attestations = Vec::with_capacity(keys.len());
        for key in keys {
            if let Some(attestation) = pending.remove(&key) {
                attestations.push(attestation);
            }
        }
        attestations
    }

    /// Return true if `peer_id_str` is currently banned (permanent or unexpired temp ban).
    pub async fn is_peer_banned(&self, peer_id_str: &str) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let banned = self.banned_peers.read().await;
        if let Some(record) = banned.get(peer_id_str) {
            if record.permanent {
                return true;
            }
            let duration_secs: u64 = match record.ban_count {
                1 => 3_600,   // 1 hour
                2 => 86_400,  // 24 hours
                3 => 604_800, // 7 days
                _ => u64::MAX,
            };
            return now < record.banned_at.saturating_add(duration_secs);
        }
        false
    }

    /// Ban a peer. Escalates ban duration on repeat offenses; permanent=true never expires.
    pub async fn ban_peer(&self, peer_id_str: &str, reason: &str, permanent: bool) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let ban_count = {
            let banned = self.banned_peers.read().await;
            banned.get(peer_id_str).map(|r| r.ban_count).unwrap_or(0) + 1
        };
        let is_permanent = permanent || ban_count >= 4;
        {
            let mut banned = self.banned_peers.write().await;
            banned.insert(
                peer_id_str.to_string(),
                BanRecord {
                    reason: reason.to_string(),
                    banned_at: now,
                    ban_count,
                    permanent: is_permanent,
                },
            );
        }
        tracing::warn!(
            peer = peer_id_str,
            reason,
            permanent = is_permanent,
            ban_count,
            "Peer banned"
        );
        self.save_ban_list().await;
    }

    async fn save_ban_list(&self) {
        let path = self.config.chain_data_dir().join("banned_peers.json");
        let banned = self.banned_peers.read().await;
        match serde_json::to_string_pretty(&*banned) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&path, &json) {
                    tracing::warn!(error = %e, "Failed to save ban list");
                }
            }
            Err(e) => tracing::warn!(error = %e, "Failed to serialize ban list"),
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
        let refiners = self.refiners.read().await;

        let mempool = self.mempool.read().await;
        let transactions: Vec<Transaction> = mempool
            .get_ordered_transactions()
            .into_iter()
            .take(MAX_TRANSACTIONS_PER_BLOCK)
            .cloned()
            .collect();

        let transaction_root = compute_transaction_root(&transactions);
        let bonded_stake = refiners.total_bonded_stake();
        let total_issued = chain.total_issued;

        let diff_target = compute_next_difficulty(
            chain.current_difficulty,
            chain.current_height,
            &chain.block_timestamps,
            total_issued,
            bonded_stake,
        );

        let difficulty = diff_target.effective_difficulty();
        let next_height = chain.current_height + 1;

        // Build the block header with all new fields
        let header = BlockHeader {
            version: BLOCK_VERSION,
            height: next_height,
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
            refiner_signature: None,
        };

        // Drain pending double-sign evidence before mining starts
        let pending_evidence: Vec<DoubleSignEvidence> = {
            let mut pending = self.pending_slash_evidence.write().await;
            std::mem::take(&mut *pending)
        };
        let pending_attestations = self.drain_pending_attestations_for_block(next_height).await;

        drop(chain);
        drop(accounts);
        drop(refiners);
        drop(mempool);

        let mut ctx = self.pow_context.write().await;
        let mut block = ctx.mine_parallel(header, difficulty, max_attempts, 0)?;
        block.slash_evidence = pending_evidence;
        block.attestations = pending_attestations;
        Some(block)
    }

    /// Produce a refiner block (signed by the node's refiner key).
    ///
    /// When `--refine` is enabled and this node's `miner_id` is the
    /// **selected** block producer (determined by weighted random sampling
    /// seeded from the previous block hash), this method builds and signs a
    /// block. The block contains no PoW proof; instead, the refiner signs
    /// the block hash with their ed25519 key, and the signature is stored in
    /// `refiner_signature`.
    ///
    /// The producer is selected via `RefinerSet::select_block_producer()`,
    /// which uses the previous block hash as entropy for deterministic,
    /// verifiable selection. Any node can verify that the producer was
    /// legitimately chosen by re-running the selection with the same seed.
    ///
    /// Returns `Some(Block)` if this node is the selected producer, or `None`
    /// if another refiner was selected or no signing key is available.
    pub async fn produce_refiner_block(&self) -> Option<Block> {
        let signing_key = self.signing_key.as_ref()?;
        let chain = self.chain.read().await;
        let refiners = self.refiners.read().await;
        let mempool = self.mempool.read().await;

        // Derive deterministic producer selection seed from the previous block hash.
        // This ensures every node computes the same producer for the same height.
        let seed = u64::from_be_bytes(
            chain.latest_block_hash.0[0..8]
                .try_into()
                .unwrap_or([0u8; 8]),
        );

        // Select the block producer via weighted random sampling
        let producer = refiners
            .select_block_producer(chain.block_timestamps.last().copied().unwrap_or(0), seed)?;

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
        let transactions: Vec<Transaction> = mempool
            .get_ordered_transactions()
            .into_iter()
            .take(MAX_TRANSACTIONS_PER_BLOCK)
            .cloned()
            .collect();

        let transaction_root = compute_transaction_root(&transactions);
        let bonded_stake = refiners.total_bonded_stake();

        let diff_target = compute_next_difficulty(
            chain.current_difficulty,
            chain.current_height,
            &chain.block_timestamps,
            chain.total_issued,
            bonded_stake,
        );
        let difficulty = diff_target.effective_difficulty();
        let next_height = chain.current_height + 1;

        // Build the block header (no PoW proof)
        let header = BlockHeader {
            version: BLOCK_VERSION,
            height: next_height,
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
            refiner_signature: None,
        };

        // Drain pending double-sign evidence into this block
        let pending_evidence: Vec<DoubleSignEvidence> = {
            let mut pending = self.pending_slash_evidence.write().await;
            std::mem::take(&mut *pending)
        };
        let pending_attestations = self.drain_pending_attestations_for_block(next_height).await;

        // Compute the block hash and sign it with the refiner's ed25519 key
        let block_hash = compute_block_hash(&header);
        let signing_payload = refiner_block_signing_payload(&block_hash);
        let signature: ed25519_dalek::Signature = signing_key.sign(&signing_payload);
        let refiner_signature = signature.to_bytes().to_vec();

        let block = Block {
            header: BlockHeader {
                refiner_signature: Some(refiner_signature),
                ..header
            },
            transactions,
            slash_evidence: pending_evidence,
            attestations: pending_attestations,
            genesis_ceremony: None,
        };

        tracing::info!(
            height = block.header.height,
            producer = %self.miner_id.to_hex(),
            "Produced refiner block"
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
        let payload_a = refiner_block_signing_payload(&evidence.hash_a);
        let payload_b = refiner_block_signing_payload(&evidence.hash_b);
        opolys_crypto::verify_ed25519(&evidence.producer_pubkey, &payload_a, &evidence.signature_a)
            && opolys_crypto::verify_ed25519(
                &evidence.producer_pubkey,
                &payload_b,
                &evidence.signature_b,
            )
    }

    fn verify_block_attestation_against_refiners(
        attestation: &BlockAttestation,
        canonical_hash: &Hash,
        refiners: &RefinerSet,
    ) -> Result<(), String> {
        if attestation.refiner_pubkey.len() != 32 {
            return Err("Attestation public key must be 32 bytes".to_string());
        }
        if attestation.signature.len() != 64 {
            return Err("Attestation signature must be 64 bytes".to_string());
        }
        if &attestation.block_hash != canonical_hash {
            return Err("Attestation block hash is not canonical".to_string());
        }

        let pk_arr: [u8; 32] = attestation
            .refiner_pubkey
            .as_slice()
            .try_into()
            .map_err(|_| "Attestation public key must be 32 bytes".to_string())?;
        let expected_refiner = opolys_crypto::ed25519_public_key_to_object_id(&pk_arr);
        if expected_refiner != attestation.refiner {
            return Err("Attestation public key does not match refiner ObjectId".to_string());
        }

        let refiner = refiners
            .get_refiner(&attestation.refiner)
            .ok_or_else(|| "Attestation refiner is not bonded".to_string())?;
        if refiner.status != RefinerStatus::Active {
            return Err("Attestation refiner is not active".to_string());
        }

        let payload =
            block_attestation_signing_payload(attestation.height, &attestation.block_hash);
        if !opolys_crypto::verify_ed25519(
            &attestation.refiner_pubkey,
            &payload,
            &attestation.signature,
        ) {
            return Err("Attestation signature verification failed".to_string());
        }

        Ok(())
    }

    fn active_refiner_weight_milli_threshold(refiners: &RefinerSet, timestamp: u64) -> u128 {
        let total_active_weight: u128 = refiners
            .all_refiners()
            .into_iter()
            .filter(|refiner| refiner.status == RefinerStatus::Active)
            .map(|refiner| refiner.weight(timestamp) as u128)
            .sum();

        (total_active_weight * FINALITY_CONFIDENCE_MILLI as u128).div_ceil(1000)
    }

    fn finalized_height_from_attestation_weights(
        current_finalized_height: u64,
        finality_weights: HashMap<u64, u128>,
        finality_threshold: u128,
    ) -> u64 {
        if finality_threshold == 0 {
            return current_finalized_height;
        }

        finality_weights
            .into_iter()
            .filter_map(|(height, weight)| {
                (height > current_finalized_height && weight >= finality_threshold)
                    .then_some(height)
            })
            .max()
            .unwrap_or(current_finalized_height)
    }

    fn cumulative_attestation_weight_for_block(
        target_height: u64,
        target_hash: &Hash,
        current_tip_height: u64,
        current_block: &Block,
        store: Option<&BlockchainStore>,
        refiners: &RefinerSet,
        timestamp: u64,
    ) -> Result<(usize, u128), String> {
        let mut seen_refiners: HashSet<ObjectId> = HashSet::new();
        let mut weight: u128 = 0;

        let mut record = |attestation: &BlockAttestation| {
            if attestation.height != target_height || &attestation.block_hash != target_hash {
                return;
            }
            if !seen_refiners.insert(attestation.refiner.clone()) {
                return;
            }
            if let Some(refiner) = refiners.get_refiner(&attestation.refiner) {
                if refiner.status == RefinerStatus::Active {
                    weight = weight.saturating_add(refiner.weight(timestamp) as u128);
                }
            }
        };

        if let Some(store) = store {
            for height in target_height.saturating_add(1)..=current_tip_height {
                let Some(block) = store
                    .load_block(height)
                    .map_err(|e| format!("Failed to load attestation block: {}", e))?
                else {
                    break;
                };
                for attestation in &block.attestations {
                    record(attestation);
                }
            }
        }

        for attestation in &current_block.attestations {
            record(attestation);
        }

        Ok((seen_refiners.len(), weight))
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
        let mut refiners = self.refiners.write().await;
        let mut mempool = self.mempool.write().await;

        let bonded_stake = refiners.total_bonded_stake();

        // Compute expected next difficulty for validation
        let expected_difficulty = compute_next_difficulty(
            chain.current_difficulty,
            chain.current_height,
            &chain.block_timestamps,
            chain.total_issued,
            bonded_stake,
        )
        .effective_difficulty();

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
        )
        .map_err(|e| format!("Block validation failed: {}", e))?;

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

        let mut processed_attestation_keys: std::collections::HashSet<(u64, String)> =
            std::collections::HashSet::new();
        let finality_threshold =
            Self::active_refiner_weight_milli_threshold(&refiners, block.header.timestamp);
        let mut finality_candidates: HashMap<u64, Hash> = HashMap::new();
        for attestation in &block.attestations {
            if attestation.height >= block.header.height {
                return Err(format!(
                    "Attestation height {} must be below block height {}",
                    attestation.height, block.header.height
                ));
            }
            let dedup_key = (attestation.height, attestation.refiner.to_hex());
            if !processed_attestation_keys.insert(dedup_key) {
                return Err(format!(
                    "Duplicate attestation from {} at height {}",
                    attestation.refiner.to_hex(),
                    attestation.height
                ));
            }
            let Some(store) = &self.store else {
                return Err("Cannot verify included attestation without storage".to_string());
            };
            let attested_block = store
                .load_block(attestation.height)
                .map_err(|e| format!("Failed to load attested block: {}", e))?
                .ok_or_else(|| "Attested block is not known locally".to_string())?;
            if attested_block.header.refiner_signature.is_none()
                || attested_block.header.pow_proof.is_some()
            {
                return Err("Attestation target must be a refiner-produced block".to_string());
            }
            let canonical_hash = compute_block_hash(&attested_block.header);
            Self::verify_block_attestation_against_refiners(
                attestation,
                &canonical_hash,
                &refiners,
            )?;
            finality_candidates
                .entry(attestation.height)
                .or_insert(canonical_hash);
            refiners.record_correct_attestation(&attestation.refiner)?;
        }

        let mut finality_weights: HashMap<u64, u128> = HashMap::new();
        for (height, hash) in finality_candidates {
            let (_count, weight) = Self::cumulative_attestation_weight_for_block(
                height,
                &hash,
                chain.current_height,
                block,
                self.store.as_deref(),
                &refiners,
                block.header.timestamp,
            )?;
            finality_weights.insert(height, weight);
        }

        let newly_finalized_height = Self::finalized_height_from_attestation_weights(
            chain.finalized_height,
            finality_weights,
            finality_threshold,
        );
        if newly_finalized_height > chain.finalized_height {
            tracing::info!(
                old_finalized_height = chain.finalized_height,
                new_finalized_height = newly_finalized_height,
                threshold_weight = finality_threshold,
                "Finalized height advanced from refiner attestations"
            );
            chain.finalized_height = newly_finalized_height;
        }

        // Verify Refiner signature if present
        if block.header.refiner_signature.is_some() && !block.header.producer.0.is_zero() {
            if let Some(ref sig_bytes) = block.header.refiner_signature {
                if sig_bytes.len() != 64 {
                    return Err("Refiner signature must be 64 bytes".to_string());
                }
                let block_hash = compute_block_hash(&block.header);
                let (pk_array, sig_array) = {
                    let account = accounts
                        .get_account(&block.header.producer)
                        .ok_or_else(|| "Refiner block producer account not found".to_string())?;
                    let pk_bytes = account.public_key.as_ref().ok_or_else(|| {
                        "Refiner block producer public key not registered".to_string()
                    })?;
                    if pk_bytes.len() != 32 {
                        return Err(
                            "Refiner block producer public key must be 32 bytes".to_string()
                        );
                    }
                    let mut pk_array = [0u8; 32];
                    pk_array.copy_from_slice(pk_bytes);
                    let mut sig_array = [0u8; 64];
                    sig_array.copy_from_slice(sig_bytes);
                    (pk_array, sig_array)
                };
                let verifying_key = ed25519_dalek::VerifyingKey::from_bytes(&pk_array)
                    .map_err(|_| "Refiner block producer public key is invalid".to_string())?;
                let signature = ed25519_dalek::Signature::from_bytes(&sig_array);
                let signing_payload = refiner_block_signing_payload(&block_hash);
                verifying_key
                    .verify(&signing_payload, &signature)
                    .map_err(|_| "Refiner signature verification failed".to_string())?;
            }
        }

        // PoW rewards can only be credited to a registered account whose
        // public key is known on-chain. This prevents arbitrary reward routing
        // to unregistered ObjectIds.
        if block.header.pow_proof.is_some() {
            let account = accounts
                .get_account(&block.header.producer)
                .ok_or_else(|| "PoW block producer account not found".to_string())?;
            match account.public_key.as_ref() {
                Some(pk) if pk.len() == 32 => {}
                Some(_) => return Err("PoW block producer public key must be 32 bytes".to_string()),
                None => return Err("PoW block producer public key not registered".to_string()),
            }
        }

        // Verify Refiner block producer was legitimately selected
        if block.header.refiner_signature.is_some() && !block.header.producer.0.is_zero() {
            let seed = u64::from_be_bytes(
                chain.latest_block_hash.0[0..8]
                    .try_into()
                    .unwrap_or([0u8; 8]),
            );
            let timestamp = chain.block_timestamps.last().copied().unwrap_or(0);
            if let Some(expected_producer) = refiners.select_block_producer(timestamp, seed) {
                if expected_producer.object_id != block.header.producer {
                    return Err(format!(
                        "Refiner block producer mismatch: expected {}, got {}",
                        expected_producer.object_id.to_hex(),
                        block.header.producer.to_hex()
                    ));
                }
            }
        }

        // Process double-sign evidence embedded in this block.
        // Each item is verified independently; duplicates within the block are skipped.
        let mut processed_evidence_keys: std::collections::HashSet<(String, u64)> =
            std::collections::HashSet::new();
        for evidence in &block.slash_evidence {
            let dedup_key = (evidence.producer.to_hex(), evidence.height);
            if processed_evidence_keys.contains(&dedup_key) {
                continue;
            }
            if Self::verify_slash_evidence(evidence) {
                processed_evidence_keys.insert(dedup_key);
                match refiners.slash_refiner(&evidence.producer, block.header.height) {
                    Ok(burned) if burned > 0 => {
                        chain.total_burned = chain.total_burned.saturating_add(burned);
                        tracing::info!(
                            producer = %evidence.producer.to_hex(),
                            burned,
                            "100% slash applied from block evidence"
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

        // Detect double-signing locally: if a refiner signed a different block at
        // the same height, build evidence for inclusion in the next mined block.
        let mut new_evidence: Vec<DoubleSignEvidence> = Vec::new();
        if block.header.refiner_signature.is_some() && !block.header.producer.0.is_zero() {
            let block_hash = compute_block_hash(&block.header);
            let sig_bytes = block.header.refiner_signature.as_ref().unwrap().clone();
            let key = (block.header.height, block.header.producer.to_hex());
            if let Some((prev_hash, prev_sig)) = chain.producer_signatures.get(&key) {
                if *prev_hash != block_hash {
                    tracing::warn!(
                        producer = %block.header.producer.to_hex(),
                        height = block.header.height,
                        "Double-sign detected locally — queuing evidence for next block"
                    );
                    let pubkey = accounts
                        .get_account(&block.header.producer)
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
                chain
                    .producer_signatures
                    .insert(key, (block_hash, sig_bytes));
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
                // Miner block: use actual hash value for vein yield calculation
                // Lucky hashes (small hash_int) earn higher yield
                pow::compute_pow_hash_value(&block.header).unwrap_or(0u64)
            } else {
                // Refiner block: refiners earn flat base reward with no luck component
                // Deliberate design: vaults earn steady fees, miners earn variable ore
                // hash_int = 0 triggers the 1.0x floor in compute_vein_yield()
                0u64
            };
            emission::compute_block_reward(
                chain.base_reward,
                block.header.difficulty,
                pow_hash_value,
            )
        };

        // Split the block reward into base and vein bonus components.
        // The coverage split (miner vs refiner) applies to base_reward ONLY.
        // The vein bonus (luck component) goes 100% to the block producer.
        // This mirrors gold: refineries charge per ounce, not per grade.
        // A rich vein doesn't increase the refinery's cut.
        let base_reward_amount = if block.header.height == 0 {
            0
        } else {
            chain.base_reward / block.header.difficulty.max(MIN_DIFFICULTY)
        };
        // Mine assay burns part of the gross ore before rewards are credited.
        // Split the net reward between miners and refiners based on stake coverage.
        // The split is on the net base reward only. The net vein bonus goes to the producer.
        // coverage_milli = (bonded_stake × 1000) / total_issued, avoiding floating point.
        let coverage_milli: u64 = if chain.total_issued > 0 {
            ((bonded_stake as u128 * 1000) / chain.total_issued as u128).min(1000) as u64
        } else {
            0
        };
        // miner_share = net_base_reward * (1000 - coverage_milli) / 1000 + net_vein_bonus
        // refiner_share = net_base_reward * coverage_milli / 1000
        let reward_distribution =
            compute_reward_distribution(block_reward, base_reward_amount, coverage_milli);
        let miner_share_amount = reward_distribution.miner_share;
        let refiner_share_amount = reward_distribution.refiner_share;

        // Credit the PoW share to the block producer (miner or selected refiner).
        // The producer is identified by block.header.producer.
        let producer = &block.header.producer;
        if !producer.0.is_zero() && miner_share_amount > 0 {
            if accounts.get_account(producer).is_none() {
                accounts.create_account(producer.clone()).ok();
            }
            accounts.credit(producer, miner_share_amount).ok();
        }

        // Distribute the refiner share among active refiners proportional to weight.
        // Each refiner's share = refiner_share_amount × (their_weight / total_weight).
        if refiner_share_amount > 0 {
            let current_timestamp = chain.block_timestamps.last().copied().unwrap_or(0);
            let total_weight = refiners.total_weight(current_timestamp);
            if total_weight > 0 {
                for v in refiners.active_refiners() {
                    let v_weight = v.weight(current_timestamp);
                    let v_share = ((refiner_share_amount as u128 * v_weight as u128)
                        / total_weight as u128) as FlakeAmount;
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

        // Execute all transactions in order
        let expected_chain_id = MAINNET_CHAIN_ID;
        let mut total_fees_burned: FlakeAmount = 0;
        for tx in &block.transactions {
            let result = TransactionDispatcher::apply_transaction(
                tx,
                &mut accounts,
                &mut refiners,
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

        // FIX 3: evict expired mempool transactions at epoch boundaries
        if block.header.height > 0 && block.header.height % opolys_core::EPOCH == 0 {
            let evicted = mempool.evict_expired(now_secs);
            if evicted > 0 {
                tracing::info!(
                    evicted,
                    height = block.header.height,
                    "Evicted expired mempool transactions at epoch boundary"
                );
            }
        }

        chain.total_burned = chain.total_burned.saturating_add(total_fees_burned);

        // Compute suggested fee for the next block via EMA of BURNED fees (not declared).
        // Fixes H3: previously used total_fees (declared) instead of total_fees_burned (actually burned),
        // which overstated the fee market signal when transactions failed.
        let next_suggested_fee = compute_suggested_fee(total_fees_burned, chain.suggested_fee);

        // Update chain state. total_issued tracks gross ore found; total_burned
        // tracks assay waste. Their difference is the net amount distributed.
        chain.total_issued = chain
            .total_issued
            .saturating_add(reward_distribution.gross_reward);
        chain.total_burned = chain
            .total_burned
            .saturating_add(reward_distribution.mine_assay);
        chain.current_height = block.header.height;
        chain.current_difficulty = block.header.difficulty;
        chain.latest_block_hash = block_hash.clone();
        chain.block_timestamps.push(block.header.timestamp);
        chain.suggested_fee = next_suggested_fee;

        // Process matured unbonding entries — return stake to accounts
        for (account, amount) in refiners.process_matured_unbonds(chain.current_height) {
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

        // Activate refiners that have been bonding for at least one epoch
        let activated = refiners.activate_matured_refiners(chain.current_height);
        if !activated.is_empty() {
            tracing::info!(
                count = activated.len(),
                "Refiners transitioned Bonding→Waiting at epoch boundary"
            );
        }

        // At epoch boundaries: rerank refiners (Waiting→Active / Active→Waiting)
        // and apply stake decay (gold vault storage fee: ~1.5%/year)
        if chain.current_height > 0 && chain.current_height % EPOCH == 0 {
            let current_ts = chain.block_timestamps.last().copied().unwrap_or(0);
            let (newly_activated, newly_demoted) = refiners.rerank_refiners(current_ts);
            if !newly_activated.is_empty() || !newly_demoted.is_empty() {
                tracing::info!(
                    activated = newly_activated.len(),
                    demoted = newly_demoted.len(),
                    height = chain.current_height,
                    "Refiner set reranked at epoch boundary"
                );
            }
            // Stake decay: burn ANNUAL_ATTRITION_PERMILLE of bonded stake per year.
            // Applied per epoch (24 hours). Mirrors gold vault storage fees.
            let decayed = refiners.decay_stake();
            if decayed > 0 {
                chain.total_burned = chain.total_burned.saturating_add(decayed);
                tracing::info!(
                    decayed,
                    height = chain.current_height,
                    "Stake decay burned at epoch boundary"
                );
            }
        }

        // No explicit consensus phase — refiners produce blocks when miners don't.
        // The coverage_milli calculation in reward distribution handles the split.

        // This state root goes into the NEXT block's header (block N+1).
        // It reflects all state changes from this block:
        // rewards, transactions, unbonds, refiner activations.
        let mut account_hasher = opolys_crypto::Blake3Hasher::new();
        account_hasher.update(DOMAIN_STATE_ROOT);
        account_hasher.update(b"chain");
        account_hasher.update(accounts.compute_state_root().as_bytes());
        account_hasher.update(refiners.compute_state_root().as_bytes());
        account_hasher.update(&chain.total_issued.to_be_bytes());
        account_hasher.update(&chain.total_burned.to_be_bytes());
        account_hasher.update(&chain.current_height.to_be_bytes());
        chain.state_root = account_hasher.finalize();

        // Persist state to disk
        if let Some(ref store) = self.store {
            if let Err(e) = Self::persist_state(store, &chain, &accounts, &refiners, block) {
                tracing::error!("Failed to persist state: {}", e);
            }
        }

        // Release all write locks before accessing pending_slash_evidence
        drop(mempool);
        drop(refiners);
        drop(accounts);
        drop(chain);

        // Queue newly detected evidence for inclusion in the next mined block
        if !new_evidence.is_empty() {
            self.pending_slash_evidence
                .write()
                .await
                .extend(new_evidence);
        }

        Ok(block_hash)
    }

    /// Persist all chain state, accounts, refiners, and the block to RocksDB.
    fn persist_state(
        store: &BlockchainStore,
        chain: &ChainState,
        accounts: &AccountStore,
        refiners: &RefinerSet,
        block: &Block,
    ) -> Result<(), String> {
        store.save_applied_block_atomic(block, &chain.to_persisted(), accounts, refiners)
    }

    /// Retrieve a block from storage by height.
    pub fn get_block(&self, height: u64) -> Option<Block> {
        self.store.as_ref()?.load_block(height).ok()?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::Signer;

    /// Helper: create a NodeConfig that uses a temporary directory.
    fn test_config() -> (NodeConfig, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("Failed to create temp dir");
        let key_path = dir.path().join("miner.key");
        fs::write(&key_path, [11u8; 32]).expect("Failed to write test miner key");
        let config = NodeConfig {
            listen_port: 0,
            rpc_port: 0,
            data_dir: dir.path().to_string_lossy().to_string(),
            bootstrap_peers: vec![],
            no_bootstrap: true,
            log_level: "warn".to_string(),
            mine: true,
            no_rpc: true,
            refine: false,
            key_file: Some(key_path.to_string_lossy().to_string()),
            rpc_listen_addr: "127.0.0.1".to_string(),
            rpc_api_key: None,
            genesis_params_path: None,
        };
        (config, dir)
    }

    async fn register_test_miner_account(node: &OpolysNode) {
        let signing_key = node.signing_key.as_ref().expect("test node has key");
        let public_key = signing_key.verifying_key().as_bytes().to_vec();
        let mut accounts = node.accounts.write().await;
        if accounts.get_account(&node.miner_id).is_none() {
            accounts.create_account(node.miner_id.clone()).unwrap();
        }
        accounts.get_account_mut(&node.miner_id).unwrap().public_key = Some(public_key);
    }

    async fn activate_test_refiner(node: &OpolysNode) {
        let mut refiners = node.refiners.write().await;
        refiners
            .bond(node.miner_id.clone(), MIN_BOND_STAKE, 0, 0)
            .unwrap();
        refiners.activate(&node.miner_id, 1).unwrap();
    }

    async fn produce_and_apply_test_refiner_block(node: &OpolysNode) -> (Block, Hash) {
        {
            let mut chain = node.chain.write().await;
            chain.current_difficulty = MIN_DIFFICULTY;
            if let Some(timestamp) = chain.block_timestamps.last_mut() {
                *timestamp = 0;
            }
        }
        let block = node
            .produce_refiner_block()
            .await
            .expect("single active refiner should be selected");
        let hash = node
            .apply_block(&block)
            .await
            .expect("refiner block should apply");
        (block, hash)
    }

    async fn produce_test_refiner_block(node: &OpolysNode) -> Block {
        {
            let mut chain = node.chain.write().await;
            if let Some(timestamp) = chain.block_timestamps.last_mut() {
                *timestamp = 0;
            }
        }
        node.produce_refiner_block()
            .await
            .expect("single active refiner should be selected")
    }

    fn write_test_genesis_attestation(dir: &tempfile::TempDir, base_reward_flakes: u64) -> String {
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]);
        let operator_public_key = hex::encode(signing_key.verifying_key().as_bytes());
        let hash_hex = hex::encode([9u8; 32]);
        let base_reward_opl = base_reward_flakes / FLAKES_PER_OPL;
        let median_production_tonnes =
            ((base_reward_opl as f64 + 0.5) * 350_640.0) / CEREMONY_TROY_OZ_PER_TONNE;
        let mut attestation = serde_json::json!({
            "ceremony_start_ms": 1_000_000u64,
            "ceremony_end_ms": 1_001_000u64,
            "ceremony_timestamp": 1_700_000_000u64,
            "operator_name": "Test Operator",
            "operator_public_key": operator_public_key,
            "operator_signature": "",
            "production_data_year": 2025u32,
            "production_sources": [
                {
                    "name": "USGS",
                    "url": "https://example.invalid/usgs",
                    "fetched_at_ms": 1_000_100u64,
                    "raw_response_hash": hash_hex,
                    "extracted_value": median_production_tonnes,
                    "status": "ok",
                    "value_origin": "auto-parse"
                },
                {
                    "name": "World Gold Council",
                    "url": "https://example.invalid/wgc",
                    "fetched_at_ms": 1_000_200u64,
                    "raw_response_hash": hex::encode([8u8; 32]),
                    "extracted_value": median_production_tonnes,
                    "status": "ok",
                    "value_origin": "auto-parse"
                },
                {
                    "name": "Kitco Production",
                    "url": "https://example.invalid/kitco",
                    "fetched_at_ms": 1_000_300u64,
                    "raw_response_hash": hex::encode([7u8; 32]),
                    "extracted_value": median_production_tonnes,
                    "status": "ok",
                    "value_origin": "auto-parse"
                },
                {
                    "name": "LBMA Annual Survey",
                    "url": "https://example.invalid/lbma-survey",
                    "fetched_at_ms": 1_000_400u64,
                    "raw_response_hash": hex::encode([5u8; 32]),
                    "extracted_value": median_production_tonnes,
                    "status": "ok",
                    "value_origin": "auto-parse"
                },
                {
                    "name": "Metals Focus",
                    "url": "https://example.invalid/metals-focus",
                    "fetched_at_ms": 1_000_500u64,
                    "raw_response_hash": hex::encode([4u8; 32]),
                    "extracted_value": median_production_tonnes,
                    "status": "ok",
                    "value_origin": "auto-parse"
                }
            ],
            "price_sources": [
                {
                    "name": "LBMA Live Price",
                    "url": "https://example.invalid/lbma",
                    "fetched_at_ms": 1_000_300u64,
                    "raw_response_hash": hex::encode([6u8; 32]),
                    "extracted_value": 2386.0,
                    "status": "ok",
                    "value_origin": "auto-parse"
                }
            ],
            "price_fetch_spread_ms": 0u64,
            "median_production_tonnes": median_production_tonnes,
            "median_price_usd_cents": 238600u64,
            "blocks_per_year": 350640u64,
            "base_reward_opl": base_reward_opl,
            "base_reward_flakes": base_reward_flakes,
            "derivation_steps": [
                format!("median_production = {:.4}", median_production_tonnes),
                "blocks_per_year = 350640"
            ],
            "master_hash": ""
        });
        let unsigned = serde_json::to_string_pretty(&attestation).unwrap();
        let master_hash = compute_ceremony_master_hash(&unsigned).unwrap();
        let signature = signing_key.sign(&master_hash);
        attestation["master_hash"] = serde_json::Value::String(hex::encode(master_hash));
        attestation["operator_signature"] =
            serde_json::Value::String(hex::encode(signature.to_bytes()));

        let path = dir.path().join("genesis_attestation.json");
        fs::write(&path, serde_json::to_string_pretty(&attestation).unwrap()).unwrap();
        path.to_string_lossy().to_string()
    }

    fn resign_test_genesis_attestation(attestation: &mut serde_json::Value) {
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]);
        attestation["master_hash"] = serde_json::Value::String(String::new());
        attestation["operator_signature"] = serde_json::Value::String(String::new());
        let unsigned = serde_json::to_string_pretty(attestation).unwrap();
        let master_hash = compute_ceremony_master_hash(&unsigned).unwrap();
        let signature = signing_key.sign(&master_hash);
        attestation["master_hash"] = serde_json::Value::String(hex::encode(master_hash));
        attestation["operator_signature"] =
            serde_json::Value::String(hex::encode(signature.to_bytes()));
    }

    #[test]
    fn node_initialization() {
        let (config, _dir) = test_config();
        let node = OpolysNode::new(config);
        assert_eq!(node.chain.blocking_read().current_height, 0);
    }

    #[test]
    fn node_config_uses_mainnet_data_subdir() {
        let (config, _dir) = test_config();
        assert!(config.chain_data_dir().ends_with("mainnet"));
        assert_eq!(config.chain_id(), MAINNET_CHAIN_ID);
    }

    #[test]
    fn reward_distribution_credits_only_net_after_assay() {
        let distribution = compute_reward_distribution(1_000_000, 800_000, 250);

        assert_eq!(distribution.mine_assay, 15_000);
        assert_eq!(distribution.net_reward, 985_000);
        assert_eq!(
            distribution.miner_share + distribution.refiner_share,
            985_000
        );
        assert_eq!(
            distribution.gross_reward - distribution.mine_assay,
            distribution.miner_share + distribution.refiner_share
        );
    }

    #[test]
    fn reward_distribution_caps_coverage_and_preserves_vein_for_producer() {
        let distribution = compute_reward_distribution(2_000_000, 1_000_000, 1_500);

        assert_eq!(distribution.mine_assay, 30_000);
        assert_eq!(distribution.net_reward, 1_970_000);
        assert_eq!(distribution.miner_share, 985_000);
        assert_eq!(distribution.refiner_share, 985_000);
        assert_eq!(
            distribution.miner_share + distribution.refiner_share,
            1_970_000
        );
    }

    #[test]
    fn genesis_attestation_loads_and_verifies() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_test_genesis_attestation(&dir, 333 * FLAKES_PER_OPL);
        let config = load_genesis_config_from_attestation(&path).unwrap();

        assert_eq!(config.base_reward, 333 * FLAKES_PER_OPL);
        assert_eq!(config.attestation.annual_production_tonnes, 3637);
        assert!(config.ceremony_data.is_some());
    }

    #[test]
    fn genesis_attestation_rejects_bad_reward_derivation() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_test_genesis_attestation(&dir, 333 * FLAKES_PER_OPL);
        let mut attestation: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        attestation["base_reward_flakes"] = serde_json::Value::from(334 * FLAKES_PER_OPL);
        attestation["base_reward_opl"] = serde_json::Value::from(334);
        resign_test_genesis_attestation(&mut attestation);
        fs::write(&path, serde_json::to_string_pretty(&attestation).unwrap()).unwrap();

        let err = load_genesis_config_from_attestation(&path).unwrap_err();
        assert!(err.contains("base_reward_flakes mismatch"), "{}", err);
    }

    #[test]
    fn genesis_attestation_rejects_manual_source_without_evidence() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_test_genesis_attestation(&dir, 333 * FLAKES_PER_OPL);
        let mut attestation: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        attestation["production_sources"][0]["status"] = serde_json::Value::String("manual".into());
        attestation["production_sources"][0]["value_origin"] =
            serde_json::Value::String("manual-after-auto-fail".into());
        attestation["production_sources"][0]["evidence_note"] = serde_json::Value::Null;
        attestation["production_sources"][0]["evidence_timestamp_ms"] = serde_json::Value::Null;
        resign_test_genesis_attestation(&mut attestation);
        fs::write(&path, serde_json::to_string_pretty(&attestation).unwrap()).unwrap();

        let err = load_genesis_config_from_attestation(&path).unwrap_err();
        assert!(err.contains("missing evidence"), "{}", err);
    }

    #[test]
    fn node_uses_ceremony_base_reward_at_genesis() {
        let (mut config, _node_dir) = test_config();
        let ceremony_dir = tempfile::tempdir().unwrap();
        config.genesis_params_path = Some(write_test_genesis_attestation(
            &ceremony_dir,
            333 * FLAKES_PER_OPL,
        ));
        let node = OpolysNode::new(config);

        assert_eq!(node.chain.blocking_read().base_reward, 333 * FLAKES_PER_OPL);
    }

    #[test]
    #[should_panic(expected = "persisted genesis")]
    fn node_refuses_persisted_state_from_different_genesis() {
        let (mut config, _node_dir) = test_config();
        let ceremony_a = tempfile::tempdir().unwrap();
        let ceremony_b = tempfile::tempdir().unwrap();
        config.genesis_params_path = Some(write_test_genesis_attestation(
            &ceremony_a,
            333 * FLAKES_PER_OPL,
        ));

        {
            let node = OpolysNode::new(config.clone());
            let persisted = node.chain.blocking_read().to_persisted();
            node.store
                .as_ref()
                .unwrap()
                .save_chain_state(&persisted)
                .unwrap();
        }

        config.genesis_params_path = Some(write_test_genesis_attestation(
            &ceremony_b,
            334 * FLAKES_PER_OPL,
        ));
        let _node = OpolysNode::new(config);
    }

    #[test]
    #[should_panic(expected = "persisted chain_id")]
    fn node_refuses_persisted_state_from_different_chain_id() {
        let (mut config, _node_dir) = test_config();
        let ceremony_dir = tempfile::tempdir().unwrap();
        config.genesis_params_path = Some(write_test_genesis_attestation(
            &ceremony_dir,
            333 * FLAKES_PER_OPL,
        ));

        let mut persisted = ChainState::new(&genesis_config_for_node(&config)).to_persisted();
        persisted.chain_id = MAINNET_CHAIN_ID + 1;
        let store = BlockchainStore::open(&config.chain_data_dir()).unwrap();
        store.save_chain_state(&persisted).unwrap();
        drop(store);

        let _node = OpolysNode::new(config);
    }

    #[test]
    fn rpc_api_key_conflicts_with_no_rpc_auth() {
        let parsed = Args::try_parse_from([
            "opolys",
            "--genesis-params",
            "genesis.json",
            "--rpc-api-key",
            "secret",
            "--no-rpc-auth",
        ]);
        assert!(parsed.is_err());
    }

    #[test]
    fn no_rpc_auth_flag_parses_explicitly() {
        let parsed = Args::try_parse_from([
            "opolys",
            "--genesis-params",
            "genesis.json",
            "--no-rpc-auth",
        ])
        .unwrap();
        assert!(parsed.no_rpc_auth);
        assert!(parsed.rpc_api_key.is_none());
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
        assert_eq!(restored.genesis_hash, chain.genesis_hash);
        assert_eq!(restored.latest_block_hash, chain.latest_block_hash);
        assert_eq!(restored.state_root, chain.state_root);
    }

    /// Integration test that mines real EVO-OMAP blocks.
    /// Genesis difficulty 7 averages 128 attempts, well within max_attempts.
    #[tokio::test]
    async fn mine_and_apply_block_links_chain() {
        let (config, _dir) = test_config();
        let node = OpolysNode::new(config);
        register_test_miner_account(&node).await;
        node.chain.write().await.current_difficulty = MIN_DIFFICULTY;

        // Capture genesis hash before mining
        let genesis_hash = node.chain.read().await.latest_block_hash.clone();
        assert_ne!(
            genesis_hash,
            Hash::zero(),
            "Genesis hash must be computed, not zero"
        );

        // Mine block 1
        let block = node
            .mine_block(1_000_000)
            .await
            .expect("Should mine block 1");
        assert_eq!(block.header.height, 1);
        assert_eq!(block.header.version, BLOCK_VERSION);
        assert_eq!(
            block.header.previous_hash, genesis_hash,
            "Block 1 must reference genesis hash"
        );

        // Apply block 1
        let result = node.apply_block(&block).await;
        assert!(result.is_ok(), "Block apply should succeed: {:?}", result);

        let block1_hash = result.unwrap();
        assert_ne!(block1_hash, Hash::zero(), "Block 1 hash must be computed");
        assert_eq!(block1_hash, node.chain.read().await.latest_block_hash);

        // Mine block 2, should reference block 1
        let block2 = node
            .mine_block(1_000_000)
            .await
            .expect("Should mine block 2");
        assert_eq!(block2.header.height, 2);
        assert_eq!(
            block2.header.previous_hash, block1_hash,
            "Block 2 must reference block 1 hash"
        );
    }

    #[tokio::test]
    async fn stale_block_candidate_rejected_after_tip_advances() {
        let (config, _dir) = test_config();
        let node = OpolysNode::new(config);
        register_test_miner_account(&node).await;
        node.chain.write().await.current_difficulty = MIN_DIFFICULTY;

        let block = node
            .mine_block(1_000_000)
            .await
            .expect("Should mine block 1");
        assert_eq!(block.header.height, 1);

        node.apply_block(&block)
            .await
            .expect("First application should advance the tip");

        let stale_result = node.apply_block(&block).await;
        assert!(
            stale_result.is_err(),
            "Re-applying a previously valid block must be rejected"
        );
        assert_eq!(node.chain.read().await.current_height, 1);
    }

    #[tokio::test]
    async fn active_refiner_creates_and_accepts_block_attestation() {
        let (config, _dir) = test_config();
        let node = OpolysNode::new(config);
        register_test_miner_account(&node).await;
        activate_test_refiner(&node).await;

        let (block, block_hash) = produce_and_apply_test_refiner_block(&node).await;
        let attestation = node
            .create_block_attestation(block.header.height, &block_hash)
            .await
            .expect("active refiner should sign attestations");

        assert_eq!(attestation.refiner, node.miner_id);
        assert_eq!(attestation.height, block.header.height);
        assert_eq!(attestation.block_hash, block_hash);

        assert!(
            node.accept_block_attestation(attestation.clone())
                .await
                .unwrap()
        );
        assert!(!node.accept_block_attestation(attestation).await.unwrap());
        assert_eq!(node.pending_attestations.read().await.len(), 1);

        let included = node
            .drain_pending_attestations_for_block(block.header.height + 1)
            .await;
        assert_eq!(included.len(), 1);
        assert_eq!(node.pending_attestations.read().await.len(), 0);
    }

    #[tokio::test]
    async fn inactive_refiner_does_not_create_block_attestation() {
        let (config, _dir) = test_config();
        let node = OpolysNode::new(config);
        register_test_miner_account(&node).await;

        let block_hash = node.chain.read().await.latest_block_hash.clone();
        assert!(
            node.create_block_attestation(0, &block_hash)
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn active_refiner_does_not_attest_mined_block() {
        let (config, _dir) = test_config();
        let node = OpolysNode::new(config);
        register_test_miner_account(&node).await;
        activate_test_refiner(&node).await;
        node.chain.write().await.current_difficulty = MIN_DIFFICULTY;

        let mined_block = node
            .mine_block(1_000_000)
            .await
            .expect("Should mine block 1");
        let mined_hash = node
            .apply_block(&mined_block)
            .await
            .expect("mined block should apply");

        assert!(
            node.create_block_attestation(mined_block.header.height, &mined_hash)
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn apply_block_rejects_invalid_included_attestation() {
        let (config, _dir) = test_config();
        let node = OpolysNode::new(config);
        register_test_miner_account(&node).await;
        activate_test_refiner(&node).await;
        node.chain.write().await.current_difficulty = MIN_DIFFICULTY;

        let (refiner_block, refiner_block_hash) = produce_and_apply_test_refiner_block(&node).await;
        let mut attestation = node
            .create_block_attestation(refiner_block.header.height, &refiner_block_hash)
            .await
            .expect("active refiner should sign attestations");
        attestation.signature[0] ^= 0x01;

        let mut block = produce_test_refiner_block(&node).await;
        block.attestations.push(attestation);

        let result = node.apply_block(&block).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .contains("Attestation signature verification failed")
        );
    }

    #[tokio::test]
    async fn apply_block_counts_valid_included_attestation() {
        let (config, _dir) = test_config();
        let node = OpolysNode::new(config);
        register_test_miner_account(&node).await;
        activate_test_refiner(&node).await;
        node.chain.write().await.current_difficulty = MIN_DIFFICULTY;

        let (refiner_block, refiner_block_hash) = produce_and_apply_test_refiner_block(&node).await;
        let attestation = node
            .create_block_attestation(refiner_block.header.height, &refiner_block_hash)
            .await
            .expect("active refiner should sign attestations");

        let mut block = produce_test_refiner_block(&node).await;
        block.attestations.push(attestation);

        node.apply_block(&block)
            .await
            .expect("valid attestation should not reject the block");

        let refiners = node.refiners.read().await;
        assert_eq!(
            refiners
                .get_refiner(&node.miner_id)
                .unwrap()
                .consecutive_correct_attestations,
            1
        );
    }

    #[test]
    fn finality_threshold_requires_two_thirds_active_weight() {
        let mut refiners = RefinerSet::new();
        let a = opolys_crypto::hash_to_object_id(b"a");
        let b = opolys_crypto::hash_to_object_id(b"b");
        refiners.bond(a.clone(), MIN_BOND_STAKE, 0, 0).unwrap();
        refiners.bond(b.clone(), MIN_BOND_STAKE, 0, 0).unwrap();
        refiners.activate(&a, 1).unwrap();
        refiners.activate(&b, 1).unwrap();

        assert_eq!(
            OpolysNode::active_refiner_weight_milli_threshold(&refiners, 0),
            1_334_000
        );
    }

    #[test]
    fn finalized_height_advances_only_past_threshold_and_never_regresses() {
        let mut weights = HashMap::new();
        weights.insert(1, 666);
        weights.insert(2, 667);
        weights.insert(3, 900);

        assert_eq!(
            OpolysNode::finalized_height_from_attestation_weights(1, weights, 667),
            3
        );

        let mut stale_weights = HashMap::new();
        stale_weights.insert(1, 1_000);
        assert_eq!(
            OpolysNode::finalized_height_from_attestation_weights(2, stale_weights, 667),
            2
        );

        let mut no_threshold = HashMap::new();
        no_threshold.insert(5, 1_000);
        assert_eq!(
            OpolysNode::finalized_height_from_attestation_weights(2, no_threshold, 0),
            2
        );
    }

    /// Supply accounting invariant: total_issued - total_burned == sum(account balances)
    /// + sum(bonded stake) + sum(pending unbonding stake).
    ///
    /// At genesis with no blocks, all values are zero.
    /// After mining a block, total_issued == account balance of miner (fees burned = 0 since no txs).
    #[tokio::test]
    async fn supply_accounting_invariant() {
        let (config, _dir) = test_config();
        let node = OpolysNode::new(config);
        register_test_miner_account(&node).await;

        let verify_invariant = |chain: &ChainState,
                                accounts: &opolys_consensus::account::AccountStore,
                                refiners: &RefinerSet| {
            let account_total: FlakeAmount =
                accounts.all_accounts().iter().map(|a| a.balance).sum();
            let bonded_total: FlakeAmount = refiners
                .all_refiners()
                .iter()
                .map(|v| v.total_stake())
                .sum();
            let unbonding_total: FlakeAmount =
                refiners.unbonding_queue.iter().map(|u| u.amount).sum();
            let accounted = account_total
                .saturating_add(bonded_total)
                .saturating_add(unbonding_total);
            let net_issued = chain.total_issued.saturating_sub(chain.total_burned);
            assert_eq!(
                net_issued, accounted,
                "Supply invariant violated: net_issued={} accounted={}",
                net_issued, accounted
            );
        };

        // Check at genesis (height 0, no blocks mined)
        {
            let chain = node.chain.read().await;
            let accounts = node.accounts.read().await;
            let refiners = node.refiners.read().await;
            verify_invariant(&chain, &accounts, &refiners);
        }
    }
}
