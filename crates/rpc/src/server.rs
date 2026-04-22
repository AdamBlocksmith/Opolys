//! JSON-RPC 2.0 HTTP server for the Opolys blockchain node.
//!
//! Exposes read-only chain queries over HTTP. Transactions are submitted via
//! the gossip network, not through RPC.
//!
//! # Endpoints
//!
//! - `POST /rpc` — JSON-RPC 2.0 methods:
//!   - `opl_getBlockHeight` — current chain height
//!   - `opl_getChainInfo` — chain statistics (height, difficulty, supply, validators)
//!   - `opl_getNetworkVersion` — protocol version string
//!   - `opl_getBalance` — account balance by ObjectId (params: `[hex_object_id]`)
//!   - `opl_getAccount` — account details by ObjectId (params: `[hex_object_id]`)
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

use opolys_core::FlakeAmount;
use opolys_consensus::account::AccountStore;
use opolys_consensus::pos::ValidatorSet;

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
}

// ─── Shared state for RPC handlers ────────────────────────────────

/// Shared state accessible to all RPC handlers.
///
/// Holds the node's live state behind async RwLocks so RPC
/// handlers can read current chain state without blocking mining.
#[derive(Clone)]
pub struct RpcState {
    pub chain: Arc<RwLock<ChainInfo>>,
    /// Live account balances and nonces.
    pub accounts: Arc<RwLock<AccountStore>>,
    /// Live validator set (stake, status).
    pub validators: Arc<RwLock<ValidatorSet>>,
}

impl RpcState {
    /// Create RPC state wrapping the shared chain, account, and validator stores.
    pub fn new(
        chain: Arc<RwLock<ChainInfo>>,
        accounts: Arc<RwLock<AccountStore>>,
        validators: Arc<RwLock<ValidatorSet>>,
    ) -> Self {
        RpcState { chain, accounts, validators }
    }
}

// ─── JSON-RPC HTTP handler ─────────────────────────────────────────

/// POST /rpc — JSON-RPC 2.0 endpoint.
///
/// Supported methods:
/// - `opl_getBlockHeight` — current chain height
/// - `opl_getChainInfo` — chain statistics
/// - `opl_getNetworkVersion` — protocol version
/// - `opl_getBalance` — account balance (params: ["object_id_hex"])
/// - `opl_getAccount` — account details (params: ["object_id_hex"])
pub async fn handle_jsonrpc(
    State(state): State<RpcState>,
    Json(req): Json<JsonRpcRequest>,
) -> (StatusCode, Json<JsonRpcResponse>) {
    let result = match req.method.as_str() {
        "opl_getBlockHeight" => {
            let chain = state.chain.read().await;
            serde_json::to_value(chain.height)
        }
        "opl_getChainInfo" => {
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
            serde_json::to_value(info)
        }
        "opl_getNetworkVersion" => {
            serde_json::to_value(opolys_core::NETWORK_PROTOCOL_VERSION)
        }
        "opl_getBalance" => {
            let params = match &req.params {
                serde_json::Value::Array(arr) => arr,
                _ => {
                    let resp = JsonRpcResponse {
                        jsonrpc: "2.0".to_string(),
                        result: None,
                        error: Some(JsonRpcError::invalid_params("Expected params array with object_id")),
                        id: req.id,
                    };
                    return (StatusCode::OK, Json(resp));
                }
            };
            if params.is_empty() {
                let resp = JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    result: None,
                    error: Some(JsonRpcError::invalid_params("Missing object_id parameter")),
                    id: req.id,
                };
                return (StatusCode::OK, Json(resp));
            }
            match parse_object_id(params[0].as_str().unwrap_or("")) {
                Ok(object_id) => {
                    let accounts = state.accounts.read().await;
                    let balance = accounts.get_account(&object_id)
                        .map(|a| a.balance)
                        .unwrap_or(0);
                    serde_json::to_value(BalanceResponse {
                        object_id: object_id.to_hex(),
                        balance_flakes: balance,
                        balance_opl: format_flake(balance),
                    })
                }
                Err(e) => {
                    serde_json::to_value(serde_json::json!({ "error": e }))
                }
            }
        }
        "opl_getAccount" => {
            let params = match &req.params {
                serde_json::Value::Array(arr) => arr,
                _ => {
                    let resp = JsonRpcResponse {
                        jsonrpc: "2.0".to_string(),
                        result: None,
                        error: Some(JsonRpcError::invalid_params("Expected params array with object_id")),
                        id: req.id,
                    };
                    return (StatusCode::OK, Json(resp));
                }
            };
            if params.is_empty() {
                let resp = JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    result: None,
                    error: Some(JsonRpcError::invalid_params("Missing object_id parameter")),
                    id: req.id,
                };
                return (StatusCode::OK, Json(resp));
            }
            match parse_object_id(params[0].as_str().unwrap_or("")) {
                Ok(object_id) => {
                    let accounts = state.accounts.read().await;
                    match accounts.get_account(&object_id) {
                        Some(account) => serde_json::to_value(AccountResponse {
                            object_id: account.object_id.to_hex(),
                            balance_flakes: account.balance,
                            balance_opl: format_flake(account.balance),
                            nonce: account.nonce,
                        }),
                        None => serde_json::to_value(serde_json::json!({ "error": "Account not found" })),
                    }
                }
                Err(e) => {
                    serde_json::to_value(serde_json::json!({ "error": e }))
                }
            }
        }
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
            error: Some(JsonRpcError::internal_error(&e.to_string())),
            id: req.id,
        })),
    }
}

/// GET /health — simple health check endpoint.
pub async fn health_check() -> &'static str {
    "ok"
}

// ─── Response types ─────────────────────────────────────────────────

/// Full chain info response including supply statistics and validator data.
///
/// Circulating supply can shrink as fees are burned — Opolys has no hard cap.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainInfoResponse {
    /// Current block height.
    pub height: u64,
    /// Current difficulty (emerges from block timestamps and stake coverage).
    pub difficulty: u64,
    /// Total OPL flakes ever emitted via block rewards.
    pub total_issued: u64,
    /// Total OPL flakes permanently burned via fees.
    pub total_burned: u64,
    /// Circulating supply in flakes (total_issued - total_burned).
    pub circulating_supply: u64,
    /// Circulating supply formatted as human-readable OPL (e.g. "440.000000 OPL").
    pub circulating_supply_opl: String,
    /// Number of active validators.
    pub validator_count: usize,
    /// Total bonded stake across all validators (in flakes).
    pub bonded_stake: u64,
    /// Current consensus phase ("ProofOfWork" or "ProofOfStake").
    pub phase: String,
    /// Network protocol version string.
    pub protocol_version: String,
}

/// Balance response for a single account.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BalanceResponse {
    /// The account's ObjectId as hex.
    pub object_id: String,
    /// Balance in flakes (1 OPL = 1,000,000 flakes).
    pub balance_flakes: u64,
    /// Balance formatted as human-readable OPL.
    pub balance_opl: String,
}

/// Account details response including nonce for transaction sequencing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountResponse {
    /// The account's ObjectId as hex.
    pub object_id: String,
    /// Balance in flakes.
    pub balance_flakes: u64,
    /// Balance formatted as human-readable OPL.
    pub balance_opl: String,
    /// Current nonce (must match the next transaction's nonce).
    pub nonce: u64,
}

/// Format a flake amount as `X.YYYYYY OPL` (6 decimal places).
///
/// 1 OPL = 1,000,000 flakes. Example: `440_000_000` → `"440.000000 OPL"`.
fn format_flake(flakes: FlakeAmount) -> String {
    let opl = flakes / opolys_core::FLAKES_PER_OPL;
    let frac = flakes % opolys_core::FLAKES_PER_OPL;
    format!("{}.{:06} OPL", opl, frac)
}

/// Parse an ObjectId from a hex string.
fn parse_object_id(hex_str: &str) -> Result<opolys_core::ObjectId, String> {
    let bytes = hex::decode(hex_str).map_err(|e| format!("Invalid hex: {}", e))?;
    if bytes.len() != 32 {
        return Err(format!("Expected 32 bytes, got {}", bytes.len()));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Ok(opolys_core::ObjectId(opolys_core::Hash::from_bytes(arr)))
}

// ─── Server builder ──────────────────────────────────────────────────

/// Build and return the Axum router with all RPC routes and CORS enabled.
///
/// CORS is configured to allow all origins, methods, and headers for
/// development convenience. In production, consider restricting origins.
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
///
/// Listens for HTTP connections. JSON-RPC at POST /rpc,
/// health check at GET /health.
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
        assert!(parse_object_id("0123").is_err()); // too short
    }
}