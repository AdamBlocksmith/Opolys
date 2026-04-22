use opolys_core::{FlakeAmount, ObjectId};
use opolys_consensus::account::AccountStore;
use opolys_consensus::pos::ValidatorSet;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Instant;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockResponse {
    pub height: u64,
    pub hash: String,
    pub transaction_count: u32,
    pub difficulty: u64,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BalanceResponse {
    pub object_id: String,
    pub balance_flakes: u64,
    pub balance_opl: String,
    pub nonce: u64,
}

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
pub struct SubmitTxResponse {
    pub tx_id: String,
    pub accepted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInfoResponse {
    pub peer_id: String,
    pub address: String,
    pub connected: bool,
}

pub trait RpcContext: Send + Sync {
    fn get_block_height(&self) -> u64;
    fn get_balance(&self, account_id: &ObjectId) -> Option<FlakeAmount>;
    fn get_nonce(&self, account_id: &ObjectId) -> Option<u64>;
    fn get_chain_info(&self) -> ChainInfoResponse;
}

pub struct RateLimiter {
    requests: HashMap<String, Vec<Instant>>,
    max_per_minute: usize,
}

impl RateLimiter {
    pub fn new(max_per_minute: usize) -> Self {
        RateLimiter {
            requests: HashMap::new(),
            max_per_minute,
        }
    }

    pub fn check(&mut self, key: &str) -> bool {
        let now = Instant::now();
        let entries = self.requests.entry(key.to_string()).or_insert_with(Vec::new);
        entries.retain(|t| now.duration_since(*t).as_secs() < 60);
        if entries.len() >= self.max_per_minute {
            return false;
        }
        entries.push(now);
        true
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
    pub id: u64,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub result: Option<serde_json::Value>,
    pub error: Option<JsonRpcError>,
    pub id: u64,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
}

impl JsonRpcError {
    pub fn method_not_found() -> Self {
        JsonRpcError { code: -32601, message: "Method not found".to_string() }
    }
    pub fn invalid_params(msg: &str) -> Self {
        JsonRpcError { code: -32602, message: msg.to_string() }
    }
    pub fn internal_error(msg: &str) -> Self {
        JsonRpcError { code: -32603, message: msg.to_string() }
    }
    pub fn not_found(msg: &str) -> Self {
        JsonRpcError { code: -32001, message: msg.to_string() }
    }
    pub fn rate_exceeded() -> Self {
        JsonRpcError { code: -32002, message: "Rate limit exceeded".to_string() }
    }
}

pub struct JsonRpcServer<C: RpcContext> {
    context: Arc<RwLock<C>>,
    rate_limiter: RateLimiter,
}

use std::sync::Arc;
use tokio::sync::RwLock;
use serde::de::Error as SerdeError;

impl<C: RpcContext + 'static> JsonRpcServer<C> {
    pub fn new(context: Arc<RwLock<C>>) -> Self {
        JsonRpcServer {
            context,
            rate_limiter: RateLimiter::new(100),
        }
    }

    pub async fn handle_request(&mut self, request: JsonRpcRequest) -> JsonRpcResponse {
        if !self.rate_limiter.check("global") {
            return JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                result: None,
                error: Some(JsonRpcError::rate_exceeded()),
                id: request.id,
            };
        }

        let context = self.context.read().await;
        let result = match request.method.as_str() {
            "opl_getBlockHeight" => serde_json::to_value(context.get_block_height()),
            "opl_getChainInfo" => serde_json::to_value(context.get_chain_info()),
            "opl_getNetworkVersion" => Ok(serde_json::Value::String(opolys_core::NETWORK_PROTOCOL_VERSION.to_string())),
            _ => Err(serde_json::Error::custom("Method not found")),
        };

        match result {
            Ok(value) => JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                result: Some(value),
                error: None,
                id: request.id,
            },
            Err(_) => JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                result: None,
                error: Some(JsonRpcError::method_not_found()),
                id: request.id,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limiter_allows_within_limit() {
        let mut limiter = RateLimiter::new(5);
        for _ in 0..5 {
            assert!(limiter.check("test"));
        }
        assert!(!limiter.check("test"));
    }

    #[test]
    fn json_rpc_request_deserialization() {
        let json = r#"{"jsonrpc":"2.0","method":"opl_getBlockHeight","params":null,"id":1}"#;
        let req: JsonRpcRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.method, "opl_getBlockHeight");
    }
}