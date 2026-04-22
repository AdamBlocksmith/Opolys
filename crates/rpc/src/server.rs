//! JSON-RPC 2.0 HTTP server for the Opolys blockchain node.
//!
//! Exposes chain queries and transaction submission over HTTP.
//!
//! # Read Endpoints (query chain state)
//!
//! - `opl_getBlockHeight` — current chain height
//! - `opl_getChainInfo` — chain statistics (height, difficulty, supply, validators)
//! - `opl_getNetworkVersion` — protocol version string
//! - `opl_getBalance` — account balance by ObjectId (params: `[hex_object_id]`)
//! - `opl_getAccount` — account details by ObjectId (params: `[hex_object_id]`)
//! - `opl_getBlockByHeight` — full block at given height (params: `[height]`)
//! - `opl_getBlockByHash` — full block by Blake3 hash (params: `[hex_hash]`)
//! - `opl_getLatestBlocks` — recent blocks (params: `[count]` or null for 10)
//! - `opl_getTransaction` — transaction by ID + status (params: `[hex_tx_id]`)
//! - `opl_getMempoolStatus` — pending transaction count and fee range
//! - `opl_getSupply` — issued, burned, circulating supply breakdown
//! - `opl_getDifficulty` — current difficulty + retarget info
//! - `opl_getValidators` — active validator set with stakes and weights
//!
//! # Write Endpoints (submit to chain)
//!
//! - `opl_sendTransaction` — submit a Borsh-hex-encoded signed transaction
//!
//! # Mining Endpoints (for external miners)
//!
//! - `opl_getMiningJob` — get a block template for mining
//! - `opl_submitSolution` — submit a mined block with valid PoW
//!
//! # Other
//!
//! - `GET /health` — returns `"ok"` if the node is running

use axum::{
    extract::State,
    http::StatusCode,
    routing::{get, post},
    Json,
    Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use tower_http::cors::{Any, CorsLayer};

use opolys_core::{FlakeAmount, Block, Transaction, ObjectId, Hash, FLAKES_PER_OPL};
use opolys_consensus::account::AccountStore;
use opolys_consensus::mempool::Mempool;
use opolys_consensus::pos::ValidatorSet;
use opolys_consensus::difficulty::compute_next_difficulty;
use opolys_consensus::block::compute_block_hash;
use opolys_storage::BlockchainStore;

use crate::jsonrpc::{JsonRpcError, JsonRpcRequest, JsonRpcResponse};

/// Simplified chain info snapshot for RPC responses.
///
/// This is a copy of the chain state that RPC handlers can read without
/// depending on the node crate. The node's `main.rs` creates this from
/// `ChainState` and keeps it updated after each block.
///
/// Circulating supply can decrease over time as fees are burned — modeling
/// real gold attrition. There is no hard cap on total issuance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainInfo {
    /// Current block height (0 = genesis).
    pub height: u64,
    /// Current mining/encoding difficulty — emerges from chain state.
    pub difficulty: u64,
    /// Total OPL flakes emitted across all block rewards.
    pub total_issued: u64,
    /// Total OPL flakes permanently burned via transaction fees.
    pub total_burned: u64,
    /// Circulating supply (total_issued - total_burned).
    pub circulating_supply: u64,
    /// Blake3-256 hash of the most recent block header (hex).
    pub latest_block_hash: String,
    /// Current consensus phase: "ProofOfWork" or "ProofOfStake".
    pub phase: String,
    /// Rolling window of block timestamps for difficulty retargeting.
    pub block_timestamps: Vec<u64>,
}

// ─── Shared state for RPC handlers ────────────────────────────────

/// Shared state accessible to all RPC handlers.
///
/// Holds the node's live state behind async RwLocks so RPC
/// handlers can read current chain state without blocking mining.
/// The BlockchainStore is thread-safe (RocksDB handles concurrency).
///
/// The `block_sender` channel allows `opl_submitSolution` to pass
/// externally-mined blocks to the node for validation and application.
/// The response channel lets the handler wait for the result.
#[derive(Clone)]
pub struct RpcState {
    /// Snapshot of chain info, updated after each block.
    pub chain: Arc<RwLock<ChainInfo>>,
    /// Live account balances and nonces.
    pub accounts: Arc<RwLock<AccountStore>>,
    /// Live validator set (stake, status).
    pub validators: Arc<RwLock<ValidatorSet>>,
    /// Transaction mempool (for sendTransaction and mempool queries).
    pub mempool: Arc<RwLock<Mempool>>,
    /// Persistent storage (for historical block/tx lookups).
    pub store: Arc<BlockchainStore>,
    /// Channel for submitting externally-mined blocks to the node.
    pub block_sender: tokio::sync::mpsc::Sender<BlockSubmission>,
}

/// A block submitted by an external miner, along with a oneshot channel
/// to send the result back to the RPC handler.
pub struct BlockSubmission {
    /// The deserialized block to apply.
    pub block: Block,
    /// Channel to send the result of applying the block back to the caller.
    pub reply: tokio::sync::oneshot::Sender<BlockSubmissionResult>,
}

/// Result of applying a submitted block.
#[derive(Debug)]
pub struct BlockSubmissionResult {
    /// The block hash, if application succeeded.
    pub block_hash: Option<String>,
    /// Error message, if application failed.
    pub error: Option<String>,
}

impl RpcState {
    /// Create RPC state wrapping all shared node components.
    pub fn new(
        chain: Arc<RwLock<ChainInfo>>,
        accounts: Arc<RwLock<AccountStore>>,
        validators: Arc<RwLock<ValidatorSet>>,
        mempool: Arc<RwLock<Mempool>>,
        store: Arc<BlockchainStore>,
        block_sender: tokio::sync::mpsc::Sender<BlockSubmission>,
    ) -> Self {
        RpcState { chain, accounts, validators, mempool, store, block_sender }
    }
}

// ─── JSON-RPC HTTP handler ─────────────────────────────────────────

/// POST /rpc — JSON-RPC 2.0 endpoint.
///
/// Routes incoming JSON-RPC requests to the appropriate handler method.
pub async fn handle_jsonrpc(
    State(state): State<RpcState>,
    Json(req): Json<JsonRpcRequest>,
) -> (StatusCode, Json<JsonRpcResponse>) {
    let result = match req.method.as_str() {
        // ── Read endpoints ──
        "opl_getBlockHeight" => handle_get_block_height(&state).await,
        "opl_getChainInfo" => handle_get_chain_info(&state).await,
        "opl_getNetworkVersion" => handle_get_network_version(),
        "opl_getBalance" => handle_get_balance(&state, &req.params).await,
        "opl_getAccount" => handle_get_account(&state, &req.params).await,
        "opl_getBlockByHeight" => handle_get_block_by_height(&state, &req.params).await,
        "opl_getBlockByHash" => handle_get_block_by_hash(&state, &req.params).await,
        "opl_getLatestBlocks" => handle_get_latest_blocks(&state, &req.params).await,
        "opl_getTransaction" => handle_get_transaction(&state, &req.params).await,
        "opl_getMempoolStatus" => handle_get_mempool_status(&state).await,
        "opl_getSupply" => handle_get_supply(&state).await,
        "opl_getDifficulty" => handle_get_difficulty(&state).await,
        "opl_getValidators" => handle_get_validators(&state).await,
        // ── Write endpoints ──
        "opl_sendTransaction" => handle_send_transaction(&state, &req.params).await,
        // ── Mining endpoints ──
        "opl_getMiningJob" => handle_get_mining_job(&state).await,
        "opl_submitSolution" => handle_submit_solution(&state, &req.params).await,
        _ => {
            let resp = JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                result: None,
                error: Some(JsonRpcError::method_not_found()),
                id: req.id,
            };
            return (StatusCode::OK, Json(resp));
        }
    };

    match result {
        Ok(value) => (StatusCode::OK, Json(JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            result: Some(value),
            error: None,
            id: req.id,
        })),
        Err(e) => (StatusCode::OK, Json(JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            result: None,
            error: Some(e),
            id: req.id,
        })),
    }
}

// ─── Read endpoint handlers ────────────────────────────────────────

async fn handle_get_block_height(state: &RpcState) -> Result<serde_json::Value, JsonRpcError> {
    let chain = state.chain.read().await;
    serde_json::to_value(chain.height).map_err(|e| JsonRpcError::internal_error(&e.to_string()))
}

async fn handle_get_chain_info(state: &RpcState) -> Result<serde_json::Value, JsonRpcError> {
    let chain = state.chain.read().await;
    let validators = state.validators.read().await;
    let info = ChainInfoResponse {
        height: chain.height,
        difficulty: chain.difficulty,
        total_issued: chain.total_issued,
        total_burned: chain.total_burned,
        circulating_supply: chain.circulating_supply,
        circulating_supply_opl: format_flake(chain.circulating_supply),
        validator_count: validators.validator_count(),
        bonded_stake: validators.total_bonded_stake(),
        phase: chain.phase.clone(),
        protocol_version: opolys_core::NETWORK_PROTOCOL_VERSION.to_string(),
    };
    serde_json::to_value(info).map_err(|e| JsonRpcError::internal_error(&e.to_string()))
}

fn handle_get_network_version() -> Result<serde_json::Value, JsonRpcError> {
    serde_json::to_value(opolys_core::NETWORK_PROTOCOL_VERSION)
        .map_err(|e| JsonRpcError::internal_error(&e.to_string()))
}

async fn handle_get_balance(state: &RpcState, params: &serde_json::Value) -> Result<serde_json::Value, JsonRpcError> {
    let object_id = require_object_id(params)?;
    let accounts = state.accounts.read().await;
    let balance = accounts.get_account(&object_id)
        .map(|a| a.balance)
        .unwrap_or(0);
    serde_json::to_value(BalanceResponse {
        object_id: object_id.to_hex(),
        balance_flakes: balance,
        balance_opl: format_flake(balance),
    }).map_err(|e| JsonRpcError::internal_error(&e.to_string()))
}

async fn handle_get_account(state: &RpcState, params: &serde_json::Value) -> Result<serde_json::Value, JsonRpcError> {
    let object_id = require_object_id(params)?;
    let accounts = state.accounts.read().await;
    match accounts.get_account(&object_id) {
        Some(account) => serde_json::to_value(AccountResponse {
            object_id: account.object_id.to_hex(),
            balance_flakes: account.balance,
            balance_opl: format_flake(account.balance),
            nonce: account.nonce,
        }).map_err(|e| JsonRpcError::internal_error(&e.to_string())),
        None => Err(JsonRpcError::not_found("Account not found")),
    }
}

async fn handle_get_block_by_height(state: &RpcState, params: &serde_json::Value) -> Result<serde_json::Value, JsonRpcError> {
    let height = require_u64_param(params, "height")?;
    match state.store.load_block(height) {
        Ok(Some(block)) => serde_json::to_value(block_to_response(&block))
            .map_err(|e| JsonRpcError::internal_error(&e.to_string())),
        Ok(None) => Err(JsonRpcError::not_found(&format!("Block at height {} not found", height))),
        Err(e) => Err(JsonRpcError::internal_error(&e.to_string())),
    }
}

async fn handle_get_block_by_hash(state: &RpcState, params: &serde_json::Value) -> Result<serde_json::Value, JsonRpcError> {
    let hex_hash = require_string_param(params, "hash")?;
    let hash = parse_hash(&hex_hash)?;
    // Look up height by hash via the store's reverse index
    match state.store.load_block_by_hash(&hash) {
        Ok(Some(block)) => serde_json::to_value(block_to_response(&block))
            .map_err(|e| JsonRpcError::internal_error(&e.to_string())),
        Ok(None) => Err(JsonRpcError::not_found("Block not found")),
        Err(e) => Err(JsonRpcError::internal_error(&e.to_string())),
    }
}

async fn handle_get_latest_blocks(state: &RpcState, params: &serde_json::Value) -> Result<serde_json::Value, JsonRpcError> {
    let count = optional_u64_param(params, 10)?;
    let chain = state.chain.read().await;
    let current_height = chain.height;
    let mut blocks = Vec::new();
    let limit = count.min(50) as u64;
    for h in (0..=current_height).rev().take(limit as usize) {
        match state.store.load_block(h) {
            Ok(Some(block)) => blocks.push(block_to_response(&block)),
            Ok(None) => break,
            Err(_) => break,
        }
    }
    serde_json::to_value(blocks).map_err(|e| JsonRpcError::internal_error(&e.to_string()))
}

async fn handle_get_transaction(state: &RpcState, params: &serde_json::Value) -> Result<serde_json::Value, JsonRpcError> {
    let hex_id = require_string_param(params, "tx_id")?;
    let tx_id = parse_object_id(&hex_id)?;

    // Check mempool first
    {
        let mempool = state.mempool.read().await;
        if let Some(tx) = mempool.get_transaction(&tx_id) {
            return serde_json::to_value(TransactionResponse {
                tx_id: tx.tx_id.to_hex(),
                sender: tx.sender.to_hex(),
                action: format_action(&tx.action),
                fee_flakes: tx.fee,
                fee_opl: format_flake(tx.fee),
                nonce: tx.nonce,
                status: "pending".to_string(),
                block_height: None,
            }).map_err(|e| JsonRpcError::internal_error(&e.to_string()));
        }
    }

    // Check blockchain store
    match state.store.load_transaction(&tx_id) {
        Ok(Some((block_height, tx))) => serde_json::to_value(TransactionResponse {
            tx_id: tx.tx_id.to_hex(),
            sender: tx.sender.to_hex(),
            action: format_action(&tx.action),
            fee_flakes: tx.fee,
            fee_opl: format_flake(tx.fee),
            nonce: tx.nonce,
            status: "confirmed".to_string(),
            block_height: Some(block_height),
        }).map_err(|e| JsonRpcError::internal_error(&e.to_string())),
        Ok(None) => Err(JsonRpcError::not_found("Transaction not found")),
        Err(e) => Err(JsonRpcError::internal_error(&e.to_string())),
    }
}

async fn handle_get_mempool_status(state: &RpcState) -> Result<serde_json::Value, JsonRpcError> {
    let mempool = state.mempool.read().await;
    serde_json::to_value(MempoolStatusResponse {
        transaction_count: mempool.transaction_count(),
        total_size_bytes: mempool.total_size(),
    }).map_err(|e| JsonRpcError::internal_error(&e.to_string()))
}

async fn handle_get_supply(state: &RpcState) -> Result<serde_json::Value, JsonRpcError> {
    let chain = state.chain.read().await;
    serde_json::to_value(SupplyResponse {
        total_issued_flakes: chain.total_issued,
        total_burned_flakes: chain.total_burned,
        circulating_supply_flakes: chain.circulating_supply,
        total_issued_opl: format_flake(chain.total_issued),
        total_burned_opl: format_flake(chain.total_burned),
        circulating_supply_opl: format_flake(chain.circulating_supply),
    }).map_err(|e| JsonRpcError::internal_error(&e.to_string()))
}

async fn handle_get_difficulty(state: &RpcState) -> Result<serde_json::Value, JsonRpcError> {
    let chain = state.chain.read().await;
    let validators = state.validators.read().await;
    let bonded_stake = validators.total_bonded_stake();
    let diff_target = compute_next_difficulty(
        chain.difficulty, chain.height, &chain.block_timestamps,
        chain.total_issued, bonded_stake,
    );
    serde_json::to_value(DifficultyResponse {
        current_difficulty: chain.difficulty,
        retarget: diff_target.retarget,
        consensus_floor: diff_target.consensus_floor,
        effective_difficulty: diff_target.effective_difficulty(),
        height: chain.height,
        next_retarget_height: ((chain.height / opolys_core::RETARGET_EPOCH) + 1) * opolys_core::RETARGET_EPOCH,
    }).map_err(|e| JsonRpcError::internal_error(&e.to_string()))
}

async fn handle_get_validators(state: &RpcState) -> Result<serde_json::Value, JsonRpcError> {
    let validators = state.validators.read().await;
    let validator_list = validators.all_validators();
    let mut result = Vec::new();
    for v in validator_list {
        let total_stake = v.total_stake();
        let total_weight = v.weight(0); // approximate — would need chain timestamp for exact value
        let entries: Vec<BondEntryResponse> = v.entries.iter().map(|e| BondEntryResponse {
            bond_id: e.bond_id,
            stake_flakes: e.stake,
            stake_opl: format_flake(e.stake),
            bonded_at_height: e.bonded_at_height,
            bonded_at_timestamp: e.bonded_at_timestamp,
        }).collect();
        result.push(ValidatorResponse {
            object_id: v.object_id.to_hex(),
            entries,
            total_stake_flakes: total_stake,
            total_stake_opl: format_flake(total_stake),
            total_weight_flakes: total_weight,
            status: format!("{:?}", v.status),
            last_signed_height: v.last_signed_height,
        });
    }
    serde_json::to_value(result).map_err(|e| JsonRpcError::internal_error(&e.to_string()))
}

// ─── Write endpoint handlers ───────────────────────────────────────

async fn handle_send_transaction(state: &RpcState, params: &serde_json::Value) -> Result<serde_json::Value, JsonRpcError> {
    let hex_data = require_string_param(params, "data")?;
    let bytes = hex::decode(&hex_data).map_err(|e| JsonRpcError::invalid_params(&format!("Invalid hex: {}", e)))?;
    let tx: Transaction = borsh::from_slice(&bytes).map_err(|e| JsonRpcError::invalid_params(&format!("Invalid transaction: {}", e)))?;

    let tx_id = tx.tx_id.clone();
    let fee = tx.fee;
    let action = format_action(&tx.action);

    // Insert into mempool with priority based on fee/size ratio
    let priority = if fee > 0 { fee as f64 / bytes.len().max(1) as f64 } else { 0.0 };
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    {
        let mut mempool = state.mempool.write().await;
        mempool.add_transaction(tx, priority, timestamp)
            .map_err(|e| JsonRpcError::invalid_params(&format!("Mempool rejected: {:?}", e)))?;
    }

    serde_json::to_value(SendTransactionResponse {
        tx_id: tx_id.to_hex(),
        fee_flakes: fee,
        fee_opl: format_flake(fee),
        action,
        status: "pending".to_string(),
    }).map_err(|e| JsonRpcError::internal_error(&e.to_string()))
}

// ─── Mining endpoint handlers ──────────────────────────────────────

async fn handle_get_mining_job(state: &RpcState) -> Result<serde_json::Value, JsonRpcError> {
    let chain = state.chain.read().await;
    let validators = state.validators.read().await;
    let mempool = state.mempool.read().await;

    let bonded_stake = validators.total_bonded_stake();
    let total_issued = chain.total_issued;

    let diff_target = compute_next_difficulty(
        chain.difficulty, chain.height, &chain.block_timestamps,
        total_issued, bonded_stake,
    );
    let difficulty = diff_target.effective_difficulty();

    // Collect transactions from mempool
    let transactions: Vec<Transaction> = mempool.get_ordered_transactions()
        .into_iter()
        .take(100)
        .cloned()
        .collect();
    let transaction_root = opolys_consensus::block::compute_transaction_root(&transactions).to_hex();

    let job = MiningJobResponse {
        height: chain.height + 1,
        previous_hash: chain.latest_block_hash.clone(),
        state_root: chain.phase.clone(),
        transaction_root,
        difficulty,
        target: u64::MAX / difficulty,
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        transaction_count: transactions.len(),
    };

    serde_json::to_value(job).map_err(|e| JsonRpcError::internal_error(&e.to_string()))
}

async fn handle_submit_solution(state: &RpcState, params: &serde_json::Value) -> Result<serde_json::Value, JsonRpcError> {
    let hex_data = require_string_param(params, "block")?;
    let bytes = hex::decode(&hex_data).map_err(|e| JsonRpcError::invalid_params(&format!("Invalid hex: {}", e)))?;
    let block: Block = borsh::from_slice(&bytes).map_err(|e| JsonRpcError::invalid_params(&format!("Invalid block: {}", e)))?;

    let height = block.header.height;
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();

    let submission = BlockSubmission {
        block,
        reply: reply_tx,
    };

    state.block_sender.send(submission).await
        .map_err(|_| JsonRpcError::internal_error("Node is not accepting blocks — channel closed"))?;

    let result = reply_rx.await
        .map_err(|_| JsonRpcError::internal_error("Node did not respond to block submission"))?;

    match result.block_hash {
        Some(hash) => serde_json::to_value(SubmitSolutionResponse {
            height,
            block_hash: hash,
            status: "accepted".to_string(),
        }).map_err(|e| JsonRpcError::internal_error(&e.to_string())),
        None => Err(JsonRpcError::internal_error(
            result.error.as_deref().unwrap_or("Block application failed"),
        )),
    }
}

// ─── Helper functions ───────────────────────────────────────────────

/// Require exactly one ObjectId parameter from the params array.
fn require_object_id(params: &serde_json::Value) -> Result<ObjectId, JsonRpcError> {
    let arr = match params {
        serde_json::Value::Array(a) => a,
        _ => return Err(JsonRpcError::invalid_params("Expected params array with object_id")),
    };
    if arr.is_empty() {
        return Err(JsonRpcError::invalid_params("Missing object_id parameter"));
    }
    parse_object_id(arr[0].as_str().unwrap_or(""))
}

/// Require a u64 parameter at index 0.
fn require_u64_param(params: &serde_json::Value, name: &str) -> Result<u64, JsonRpcError> {
    let arr = match params {
        serde_json::Value::Array(a) => a,
        _ => return Err(JsonRpcError::invalid_params(&format!("Expected params array with {}", name))),
    };
    if arr.is_empty() {
        return Err(JsonRpcError::invalid_params(&format!("Missing {} parameter", name)));
    }
    arr[0].as_u64().ok_or_else(|| JsonRpcError::invalid_params(&format!("{} must be a number", name)))
}

/// Require a string parameter at index 0.
fn require_string_param(params: &serde_json::Value, name: &str) -> Result<String, JsonRpcError> {
    let arr = match params {
        serde_json::Value::Array(a) => a,
        _ => return Err(JsonRpcError::invalid_params(&format!("Expected params array with {}", name))),
    };
    if arr.is_empty() {
        return Err(JsonRpcError::invalid_params(&format!("Missing {} parameter", name)));
    }
    arr[0].as_str().map(String::from)
        .ok_or_else(|| JsonRpcError::invalid_params(&format!("{} must be a string", name)))
}

/// Optional u64 parameter, defaults to `default`.
fn optional_u64_param(params: &serde_json::Value, default: u64) -> Result<u64, JsonRpcError> {
    let arr = match params {
        serde_json::Value::Array(a) => a,
        _ => return Ok(default),
    };
    if arr.is_empty() {
        return Ok(default);
    }
    Ok(arr[0].as_u64().unwrap_or(default))
}

/// Parse a Blake3 hash from hex.
fn parse_hash(hex_str: &str) -> Result<Hash, JsonRpcError> {
    let bytes = hex::decode(hex_str).map_err(|e| JsonRpcError::invalid_params(&format!("Invalid hex: {}", e)))?;
    if bytes.len() != 32 {
        return Err(JsonRpcError::invalid_params(&format!("Expected 32-byte hash, got {} bytes", bytes.len())));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Ok(Hash::from_bytes(arr))
}

/// Parse an ObjectId from a hex string.
fn parse_object_id(hex_str: &str) -> Result<ObjectId, JsonRpcError> {
    let bytes = hex::decode(hex_str).map_err(|e| JsonRpcError::invalid_params(&format!("Invalid hex: {}", e)))?;
    if bytes.len() != 32 {
        return Err(JsonRpcError::invalid_params(&format!("Expected 32 bytes, got {}", bytes.len())));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Ok(ObjectId(Hash::from_bytes(arr)))
}

/// Format a TransactionAction as a human-readable string.
fn format_action(action: &opolys_core::TransactionAction) -> String {
    match action {
        opolys_core::TransactionAction::Transfer { recipient, amount } => {
            format!("Transfer {} flakes to {}", amount, recipient.to_hex())
        }
        opolys_core::TransactionAction::ValidatorBond { amount } => {
            format!("Bond {} flakes ({})", amount, format_flake(*amount))
        }
        opolys_core::TransactionAction::ValidatorUnbond { bond_id } => format!("Unbond entry #{}", bond_id),
    }
}

/// Convert a Block to a JSON-serializable response.
fn block_to_response(block: &Block) -> BlockResponse {
    BlockResponse {
        height: block.header.height,
        previous_hash: block.header.previous_hash.to_hex(),
        state_root: block.header.state_root.to_hex(),
        transaction_root: block.header.transaction_root.to_hex(),
        timestamp: block.header.timestamp,
        difficulty: block.header.difficulty,
        transaction_count: block.transactions.len(),
        block_hash: compute_block_hash(&block.header).to_hex(),
    }
}

// ─── Response types ─────────────────────────────────────────────────

/// Full chain info response including supply statistics and validator data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainInfoResponse {
    pub height: u64,
    pub difficulty: u64,
    pub total_issued: u64,
    pub total_burned: u64,
    pub circulating_supply: u64,
    pub circulating_supply_opl: String,
    pub validator_count: usize,
    pub bonded_stake: u64,
    pub phase: String,
    pub protocol_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BalanceResponse {
    pub object_id: String,
    pub balance_flakes: u64,
    pub balance_opl: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountResponse {
    pub object_id: String,
    pub balance_flakes: u64,
    pub balance_opl: String,
    pub nonce: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockResponse {
    pub height: u64,
    pub previous_hash: String,
    pub state_root: String,
    pub transaction_root: String,
    pub timestamp: u64,
    pub difficulty: u64,
    pub transaction_count: usize,
    pub block_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionResponse {
    pub tx_id: String,
    pub sender: String,
    pub action: String,
    pub fee_flakes: u64,
    pub fee_opl: String,
    pub nonce: u64,
    pub status: String,
    pub block_height: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MempoolStatusResponse {
    pub transaction_count: usize,
    pub total_size_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupplyResponse {
    pub total_issued_flakes: u64,
    pub total_burned_flakes: u64,
    pub circulating_supply_flakes: u64,
    pub total_issued_opl: String,
    pub total_burned_opl: String,
    pub circulating_supply_opl: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DifficultyResponse {
    pub current_difficulty: u64,
    pub retarget: u64,
    pub consensus_floor: u64,
    pub effective_difficulty: u64,
    pub height: u64,
    pub next_retarget_height: u64,
}

/// A single bond entry within a validator's stake.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BondEntryResponse {
    pub bond_id: u64,
    pub stake_flakes: u64,
    pub stake_opl: String,
    pub bonded_at_height: u64,
    pub bonded_at_timestamp: u64,
}

/// Full validator info response with per-entry bond details.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatorResponse {
    pub object_id: String,
    pub entries: Vec<BondEntryResponse>,
    pub total_stake_flakes: u64,
    pub total_stake_opl: String,
    pub total_weight_flakes: u64,
    pub status: String,
    pub last_signed_height: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendTransactionResponse {
    pub tx_id: String,
    pub fee_flakes: u64,
    pub fee_opl: String,
    pub action: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MiningJobResponse {
    pub height: u64,
    pub previous_hash: String,
    pub state_root: String,
    pub transaction_root: String,
    pub difficulty: u64,
    pub target: u64,
    pub timestamp: u64,
    pub transaction_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitSolutionResponse {
    pub height: u64,
    pub block_hash: String,
    pub status: String,
}

/// Format a flake amount as `X.YYYYYY OPL` (6 decimal places).
fn format_flake(flakes: FlakeAmount) -> String {
    let opl = flakes / FLAKES_PER_OPL;
    let frac = flakes % FLAKES_PER_OPL;
    format!("{}.{:06} OPL", opl, frac)
}

/// GET /health — simple health check endpoint.
pub async fn health_check() -> &'static str {
    "ok"
}

// ─── Server builder ──────────────────────────────────────────────────

/// Build and return the Axum router with all RPC routes and CORS enabled.
pub fn build_router(state: RpcState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
        .route("/rpc", post(handle_jsonrpc))
        .route("/health", get(health_check))
        .with_state(state)
        .layer(cors)
}

/// Start the RPC server on the given port.
pub async fn start_server(state: RpcState, port: u16) -> Result<(), anyhow::Error> {
    let app = build_router(state);
    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));

    tracing::info!("RPC server listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_flake_amounts() {
        assert_eq!(format_flake(1_000_000), "1.000000 OPL");
        assert_eq!(format_flake(0), "0.000000 OPL");
        assert_eq!(format_flake(1), "0.000001 OPL");
        assert_eq!(format_flake(440_000_000), "440.000000 OPL");
    }

    #[test]
    fn object_id_from_hex_roundtrip() {
        let id = opolys_crypto::hash_to_object_id(b"test");
        let hex = id.to_hex();
        let restored = parse_object_id(&hex).unwrap();
        assert_eq!(id, restored);
    }

    #[test]
    fn object_id_from_invalid_hex() {
        assert!(parse_object_id("not_hex").is_err());
        assert!(parse_object_id("0123").is_err());
    }
}