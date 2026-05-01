//! JSON-RPC 2.0 HTTP server for the Opolys blockchain node.
//!
//! Exposes chain queries and transaction submission over HTTP.
//!
//! # Read Endpoints (query chain state)
//!
//! - `opl_getBlockHeight` — current chain height
//! - `opl_getChainInfo` — chain statistics (height, difficulty, supply, refiners)
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
//! - `opl_getRefiners` — active refiner set with stakes and weights
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
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json,
    Router,
};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::sync::RwLock;
use tower_http::cors::{Any, CorsLayer};

use opolys_core::{FlakeAmount, Block, Transaction, ObjectId, Hash, FLAKES_PER_OPL, EPOCH, MAX_ACTIVE_REFINERS};
use opolys_consensus::account::AccountStore;
use opolys_consensus::mempool::Mempool;
use opolys_consensus::refiner::RefinerSet;
use opolys_consensus::difficulty::compute_next_difficulty;
use opolys_consensus::block::compute_block_hash;
use opolys_storage::BlockchainStore;

use crate::jsonrpc::{JsonRpcError, JsonRpcRequest, JsonRpcResponse, RateLimiter};

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
    /// Blake3-256 hash of the most recent block header.
    pub latest_block_hash: Hash,
    /// Blake3-256 hash of the state root after the most recent block.
    pub state_root: Hash,
    /// Rolling window of block timestamps for difficulty retargeting.
    pub block_timestamps: Vec<u64>,
    /// Suggested fee for the next block (Flakes), computed via EMA.
    pub suggested_fee: u64,
    /// Height of the most recently finalized block (cannot be reverted).
    pub finalized_height: u64,
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
    /// Live refiner set (stake, status).
    pub refiners: Arc<RwLock<RefinerSet>>,
    /// Transaction mempool (for sendTransaction and mempool queries).
    pub mempool: Arc<RwLock<Mempool>>,
    /// Persistent storage (for historical block/tx lookups).
    pub store: Arc<BlockchainStore>,
    /// Channel for submitting externally-mined blocks to the node.
    pub block_sender: tokio::sync::mpsc::Sender<BlockSubmission>,
    /// The miner's on-chain identity (Blake3 hash of their public key).
    /// Set to ObjectId::zero() if no key file is provided.
    pub miner_id: ObjectId,
    /// Per-IP rate limiter shared across all handlers.
    /// Three tiers keyed as "<ip>:read" (120/min), "<ip>:write" (10/min), "<ip>:mining" (30/min).
    pub rate_limiter: Arc<Mutex<RateLimiter>>,
    /// Optional API key for write and mining endpoints.
    /// If Some, opl_sendTransaction/getMiningJob/submitSolution require
    /// Authorization: Bearer <key> or X-Api-Key: <key>. Read methods always public.
    pub api_key: Option<String>,
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
        refiners: Arc<RwLock<RefinerSet>>,
        mempool: Arc<RwLock<Mempool>>,
        store: Arc<BlockchainStore>,
        block_sender: tokio::sync::mpsc::Sender<BlockSubmission>,
        miner_id: ObjectId,
        api_key: Option<String>,
    ) -> Self {
        RpcState {
            chain, accounts, refiners, mempool, store, block_sender, miner_id,
            rate_limiter: Arc::new(Mutex::new(RateLimiter::new(120))),
            api_key,
        }
    }
}

// ─── JSON-RPC HTTP handler ─────────────────────────────────────────

/// Classify a method name into its rate-limit tier and auth requirement.
///
/// Returns `(rate_key, max_per_minute, requires_api_key)`.
fn classify_method(method: &str) -> (&'static str, usize, bool) {
    match method {
        "opl_sendTransaction" => ("write", 10, true),
        "opl_getMiningJob" | "opl_submitSolution" => ("mining", 30, true),
        _ => ("read", 120, false),
    }
}

/// Verify `Authorization: Bearer <key>` or `X-Api-Key: <key>` header.
fn check_api_key(headers: &HeaderMap, required_key: &str) -> bool {
    if let Some(val) = headers.get("authorization").and_then(|v| v.to_str().ok()) {
        if val.starts_with("Bearer ") && &val["Bearer ".len()..] == required_key {
            return true;
        }
    }
    if let Some(val) = headers.get("x-api-key").and_then(|v| v.to_str().ok()) {
        if val == required_key {
            return true;
        }
    }
    false
}

/// POST /rpc — JSON-RPC 2.0 endpoint.
///
/// Routes incoming JSON-RPC requests to the appropriate handler method.
/// Applies per-IP rate limiting and optional API key authentication before routing.
pub async fn handle_jsonrpc(
    State(state): State<RpcState>,
    ConnectInfo(client_addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<JsonRpcRequest>,
) -> (StatusCode, Json<JsonRpcResponse>) {
    let ip = client_addr.ip().to_string();
    let (tier, max_per_min, needs_auth) = classify_method(&req.method);

    // Layer 2: per-IP rate limiting
    {
        let mut limiter = state.rate_limiter.lock().unwrap();
        let rate_key = format!("{}:{}", ip, tier);
        if !limiter.check_limit(&rate_key, max_per_min) {
            return (StatusCode::TOO_MANY_REQUESTS, Json(JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                result: None,
                error: Some(JsonRpcError::rate_limited()),
                id: req.id,
            }));
        }
    }

    // Layer 3: API key check for write and mining methods
    if needs_auth {
        if let Some(ref required_key) = state.api_key {
            if !check_api_key(&headers, required_key) {
                return (StatusCode::UNAUTHORIZED, Json(JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    result: None,
                    error: Some(JsonRpcError::unauthorized()),
                    id: req.id,
                }));
            }
        }
    }

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
        "opl_getRefiners" => handle_get_refiners(&state).await,
        "opl_getFinalizedHeight" => handle_get_finalized_height(&state).await,
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
    let refiners = state.refiners.read().await;
    let info = ChainInfoResponse {
        height: chain.height,
        difficulty: chain.difficulty,
        total_issued: chain.total_issued,
        total_burned: chain.total_burned,
        circulating_supply: chain.circulating_supply,
        circulating_supply_opl: format_flake(chain.circulating_supply),
        suggested_fee: chain.suggested_fee,
        suggested_fee_opl: format_flake(chain.suggested_fee),
        refiner_count: refiners.refiner_count(),
        active_refiners: refiners.total_active_refiners(),
        bonding_refiners: refiners.total_bonding_refiners(),
        waiting_refiners: refiners.total_waiting_refiners(),
        max_active_refiners: MAX_ACTIVE_REFINERS,
        bonded_stake: refiners.total_bonded_stake(),
        protocol_version: opolys_core::NETWORK_PROTOCOL_VERSION.to_string(),
        finalized_height: chain.finalized_height,
    };
    serde_json::to_value(info).map_err(|e| JsonRpcError::internal_error(&e.to_string()))
}

async fn handle_get_finalized_height(state: &RpcState) -> Result<serde_json::Value, JsonRpcError> {
    let chain = state.chain.read().await;
    serde_json::to_value(chain.finalized_height).map_err(|e| JsonRpcError::internal_error(&e.to_string()))
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
    let finalized_height = state.chain.read().await.finalized_height;
    match state.store.load_block(height) {
        Ok(Some(block)) => serde_json::to_value(block_to_response(&block, finalized_height))
            .map_err(|e| JsonRpcError::internal_error(&e.to_string())),
        Ok(None) => Err(JsonRpcError::not_found(&format!("Block at height {} not found", height))),
        Err(e) => Err(JsonRpcError::internal_error(&e.to_string())),
    }
}

async fn handle_get_block_by_hash(state: &RpcState, params: &serde_json::Value) -> Result<serde_json::Value, JsonRpcError> {
    let hex_hash = require_string_param(params, "hash")?;
    let hash = parse_hash(&hex_hash)?;
    let finalized_height = state.chain.read().await.finalized_height;
    // Look up height by hash via the store's reverse index
    match state.store.load_block_by_hash(&hash) {
        Ok(Some(block)) => serde_json::to_value(block_to_response(&block, finalized_height))
            .map_err(|e| JsonRpcError::internal_error(&e.to_string())),
        Ok(None) => Err(JsonRpcError::not_found("Block not found")),
        Err(e) => Err(JsonRpcError::internal_error(&e.to_string())),
    }
}

async fn handle_get_latest_blocks(state: &RpcState, params: &serde_json::Value) -> Result<serde_json::Value, JsonRpcError> {
    let count = optional_u64_param(params, 10)?;
    let chain = state.chain.read().await;
    let current_height = chain.height;
    let finalized_height = chain.finalized_height;
    drop(chain);
    let mut blocks = Vec::new();
    let limit = count.min(50) as u64;
    for h in (0..=current_height).rev().take(limit as usize) {
        match state.store.load_block(h) {
            Ok(Some(block)) => blocks.push(block_to_response(&block, finalized_height)),
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
    let refiners = state.refiners.read().await;
    let bonded_stake = refiners.total_bonded_stake();
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
        next_retarget_height: ((chain.height / EPOCH) + 1) * EPOCH,
    }).map_err(|e| JsonRpcError::internal_error(&e.to_string()))
}

async fn handle_get_refiners(state: &RpcState) -> Result<serde_json::Value, JsonRpcError> {
    let refiners = state.refiners.read().await;
    let chain = state.chain.read().await;
    let current_timestamp = chain.block_timestamps.last().copied().unwrap_or(0);
    let refiner_list = refiners.all_refiners();
    let mut result = Vec::new();
    for v in refiner_list {
        let total_stake = v.total_stake();
        let total_weight = v.weight(current_timestamp);
        let entries: Vec<BondEntryResponse> = v.entries.iter().map(|e| BondEntryResponse {
            stake_flakes: e.stake,
            stake_opl: format_flake(e.stake),
            bonded_at_height: e.bonded_at_height,
            bonded_at_timestamp: e.bonded_at_timestamp,
        }).collect();
        result.push(RefinerResponse {
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

    let sender = tx.sender.clone();
    let account_nonce = state.accounts.read().await
        .get_account(&sender)
        .map(|a| a.nonce)
        .unwrap_or(0);
    let suggested_fee = state.chain.read().await.suggested_fee;

    {
        let mut mempool = state.mempool.write().await;
        mempool.add_transaction(tx, priority, timestamp, account_nonce, suggested_fee)
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
    let refiners = state.refiners.read().await;
    let mempool = state.mempool.read().await;

    let bonded_stake = refiners.total_bonded_stake();
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
    let transaction_root = opolys_consensus::block::compute_transaction_root(&transactions);
    let transaction_root_hex = transaction_root.to_hex();

    // Build a template block header for mining
    let header = opolys_core::BlockHeader {
        version: opolys_core::BLOCK_VERSION,
        height: chain.height + 1,
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
        producer: state.miner_id.clone(),
        pow_proof: None,
        refiner_signature: None,
    };

    // Pre-serialize the header for EVO-OMAP mining
    let header_bytes = opolys_consensus::pow::serialize_header_for_pow(&header);
    let header_bytes_hex = hex::encode(&header_bytes);

    // Convert difficulty to u64 target using EVO-OMAP leading-zero-bits model
    let target = opolys_consensus::emission::difficulty_to_target(difficulty);

    let job = MiningJobResponse {
        version: opolys_core::BLOCK_VERSION,
        height: chain.height + 1,
        previous_hash: chain.latest_block_hash.to_hex(),
        state_root: chain.state_root.to_hex(),
        transaction_root: transaction_root_hex,
        difficulty,
        target,
        suggested_fee: chain.suggested_fee,
        timestamp: header.timestamp,
        transaction_count: transactions.len(),
        producer: state.miner_id.to_hex(),
        header_bytes: header_bytes_hex,
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
        opolys_core::TransactionAction::RefinerBond { amount } => {
            format!("Bond {} flakes ({})", amount, format_flake(*amount))
        }
        opolys_core::TransactionAction::RefinerUnbond { amount } => format!("Unbond {} flakes ({})", amount, format_flake(*amount)),
    }
}

/// Convert a Block to a JSON-serializable response.
fn block_to_response(block: &Block, finalized_height: u64) -> BlockResponse {
    BlockResponse {
        version: block.header.version,
        height: block.header.height,
        previous_hash: block.header.previous_hash.to_hex(),
        state_root: block.header.state_root.to_hex(),
        transaction_root: block.header.transaction_root.to_hex(),
        timestamp: block.header.timestamp,
        difficulty: block.header.difficulty,
        suggested_fee: block.header.suggested_fee,
        transaction_count: block.transactions.len(),
        block_hash: compute_block_hash(&block.header).to_hex(),
        finalized: block.header.height <= finalized_height,
    }
}

// ─── Response types ─────────────────────────────────────────────────

/// Full chain info response including supply statistics and refiner data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainInfoResponse {
    pub height: u64,
    pub difficulty: u64,
    pub total_issued: u64,
    pub total_burned: u64,
    pub circulating_supply: u64,
    pub circulating_supply_opl: String,
    pub suggested_fee: u64,
    pub suggested_fee_opl: String,
    /// Total refiners regardless of status (Active + Bonding + Slashed).
    pub refiner_count: usize,
    /// Refiners currently in Active status (producing blocks, earning rewards).
    pub active_refiners: usize,
    /// Refiners in Bonding status — waiting for epoch maturity before joining Waiting pool.
    pub bonding_refiners: usize,
    /// Refiners in Waiting status — eligible but outside the top-N active set by weight.
    pub waiting_refiners: usize,
    /// Protocol cap on simultaneously Active refiners.
    pub max_active_refiners: usize,
    pub bonded_stake: u64,
    pub protocol_version: String,
    /// Height of the most recently finalized block (cannot be reverted).
    pub finalized_height: u64,
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
    pub version: u32,
    pub height: u64,
    pub previous_hash: String,
    pub state_root: String,
    pub transaction_root: String,
    pub timestamp: u64,
    pub difficulty: u64,
    pub suggested_fee: u64,
    pub transaction_count: usize,
    pub block_hash: String,
    /// True if this block cannot be reverted.
    pub finalized: bool,
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

/// A single bond entry within a refiner's stake.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BondEntryResponse {
    pub stake_flakes: u64,
    pub stake_opl: String,
    pub bonded_at_height: u64,
    pub bonded_at_timestamp: u64,
}

/// Full refiner info response with per-entry bond details.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefinerResponse {
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
    /// Protocol version for the block template.
    pub version: u32,
    /// Height of the next block to mine.
    pub height: u64,
    /// Blake3-256 hash of the previous block header (hex).
    pub previous_hash: String,
    /// Blake3-256 hash of the current state root (hex).
    pub state_root: String,
    /// Blake3-256 Merkle root of the proposed transactions (hex).
    pub transaction_root: String,
    /// EVO-OMAP difficulty (leading zero bits required in SHA3-256 hash).
    /// This is NOT a u64 divisor — difficulty D means the hash must have
    /// at least D leading zero bits. For vein yield, the corresponding
    /// u64 target is `target` (see below).
    pub difficulty: u64,
    /// u64 hash target derived from difficulty using leading-zero-bits model:
    /// `target = 2^(64-D) - 1`. A valid block has SHA3-256 hash where the
    /// first 8 bytes (as u64 big-endian) are <= this target.
    pub target: u64,
    /// Suggested fee for the next block (Flakes), computed via EMA.
    pub suggested_fee: u64,
    /// Unix timestamp (seconds) for the block template.
    pub timestamp: u64,
    /// Number of transactions in the template.
    pub transaction_count: usize,
    /// ObjectId (hex) of the block producer — miner's on-chain identity.
    /// Blake3(public_key) == ObjectId. The miner must include this in the
    /// submitted block so the block reward is credited correctly.
    pub producer: String,
    /// Pre-serialized header bytes for EVO-OMAP mining. Miners append
    /// the 8-byte nonce and compute EVO-OMAP hash over this. This
    /// eliminates the need for miners to re-serialize the header.
    pub header_bytes: String,
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

/// Start the RPC server on the given port and listen address.
///
/// Defaults to `127.0.0.1` (localhost-only). Pass `0.0.0.0` via
/// `--rpc-listen-addr` to expose publicly. Uses `into_make_service_with_connect_info`
/// so handlers can extract the client IP for per-IP rate limiting.
pub async fn start_server(state: RpcState, port: u16, listen_addr: &str) -> Result<(), anyhow::Error> {
    let ip: std::net::IpAddr = listen_addr.parse()
        .map_err(|e| anyhow::anyhow!("Invalid --rpc-listen-addr '{}': {}", listen_addr, e))?;
    let addr = SocketAddr::from((ip, port));

    tracing::info!("RPC server listening on {}", addr);

    let app = build_router(state);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await?;

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
        assert_eq!(format_flake(312_000_000), "312.000000 OPL");
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