//! JSON-RPC 2.0 protocol types and rate limiting for the Opolys RPC server.
//!
//! Implements the standard JSON-RPC 2.0 request/response envelope with
//! Opolys-specific error codes. Also provides a simple per-client rate
//! limiter to prevent RPC abuse.

use serde::{Deserialize, Serialize};
use std::time::Instant;

/// JSON-RPC 2.0 request format.
///
/// All Opolys RPC methods use JSON-RPC 2.0 over HTTP POST to `/rpc`.
/// The `params` field is typically an array of arguments (e.g. `["hex_object_id"]`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    /// Must be `"2.0"`.
    pub jsonrpc: String,
    /// The method name (e.g. `"opl_getBalance"`, `"opl_getChainInfo"`).
    pub method: String,
    /// Method parameters — defaults to `Null` if omitted.
    #[serde(default = "default_params")]
    pub params: serde_json::Value,
    /// Request ID for matching responses to requests.
    pub id: u64,
}

/// Default `params` value for deserialization when the field is missing.
fn default_params() -> serde_json::Value {
    serde_json::Value::Null
}

/// JSON-RPC 2.0 response format.
///
/// On success, `result` is set and `error` is `None`. On failure,
/// `error` is set and `result` is `None`.
#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcResponse {
    /// Must be `"2.0"`.
    pub jsonrpc: String,
    /// The result payload on success.
    pub result: Option<serde_json::Value>,
    /// The error payload on failure.
    pub error: Option<JsonRpcError>,
    /// Matches the request ID.
    pub id: u64,
}

/// JSON-RPC error object with standard error codes.
///
/// Opolys uses standard JSON-RPC error codes:
/// - `-32600` Invalid request
/// - `-32601` Method not found
/// - `-32602` Invalid params
/// - `-32603` Internal error
/// - `-32001` Not found (application-specific)
#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcError {
    /// Numeric error code (JSON-RPC 2.0 standard or application-specific).
    pub code: i32,
    /// Human-readable error description.
    pub message: String,
}

impl JsonRpcError {
    /// `-32600` — the request envelope is not a valid JSON-RPC 2.0 request.
    pub fn invalid_request(msg: &str) -> Self {
        JsonRpcError {
            code: -32600,
            message: msg.to_string(),
        }
    }

    /// `-32601` — the requested method does not exist.
    pub fn method_not_found() -> Self {
        JsonRpcError {
            code: -32601,
            message: "Method not found".to_string(),
        }
    }

    /// `-32602` — invalid or missing method parameters.
    pub fn invalid_params(msg: &str) -> Self {
        JsonRpcError {
            code: -32602,
            message: msg.to_string(),
        }
    }

    /// `-32603` — an internal server error occurred.
    pub fn internal_error(msg: &str) -> Self {
        JsonRpcError {
            code: -32603,
            message: msg.to_string(),
        }
    }

    /// `-32001` — the requested resource was not found (application-specific).
    pub fn not_found(msg: &str) -> Self {
        JsonRpcError {
            code: -32001,
            message: msg.to_string(),
        }
    }

    /// `-32004` — authentication required (application-specific).
    pub fn unauthorized() -> Self {
        JsonRpcError {
            code: -32004,
            message: "This method requires an API key. \
                      Use Authorization: Bearer <key> or X-Api-Key: <key>."
                .to_string(),
        }
    }

    /// `-32005` — rate limit exceeded (application-specific).
    pub fn rate_limited() -> Self {
        JsonRpcError {
            code: -32005,
            message: "Rate limit exceeded. Too many requests.".to_string(),
        }
    }
}

/// Rate limiter for RPC request throttling.
///
/// Tracks request counts per client key within a 60-second window.
/// If a client exceeds `max_per_minute` requests, further requests
/// are rejected until the window expires.
/// Per-client rate limiter for RPC request throttling.
///
/// Tracks request timestamps per client key within a rolling 60-second window.
/// If a client exceeds `max_per_minute` requests, further requests are
/// rejected until the window expires. This prevents abuse of the public
/// RPC endpoint.
pub struct RateLimiter {
    /// Map from client identifier to list of request timestamps within the current window.
    requests: std::collections::HashMap<String, Vec<Instant>>,
    /// Maximum number of requests allowed per client per 60-second window.
    max_per_minute: usize,
}

impl RateLimiter {
    /// Create a rate limiter allowing `max_per_minute` requests per client per minute.
    pub fn new(max_per_minute: usize) -> Self {
        RateLimiter {
            requests: std::collections::HashMap::new(),
            max_per_minute,
        }
    }

    /// Check whether a request from the given key is allowed.
    ///
    /// Removes expired entries (older than 60 seconds) from the client's
    /// history, then checks if the count is still within `max_per_minute`.
    /// Returns `true` if allowed, `false` if the rate limit has been exceeded.
    pub fn check(&mut self, key: &str) -> bool {
        self.check_limit(key, self.max_per_minute)
    }

    /// Check whether a request from the given key is allowed with a specific limit.
    ///
    /// Like `check` but overrides `max_per_minute` with `max`. Used to apply
    /// different rate limits to different method tiers from a single limiter instance.
    pub fn check_limit(&mut self, key: &str, max: usize) -> bool {
        let now = Instant::now();
        let entries = self
            .requests
            .entry(key.to_string())
            .or_insert_with(Vec::new);
        entries.retain(|t| now.duration_since(*t).as_secs() < 60);
        if entries.len() >= max {
            return false;
        }
        entries.push(now);
        true
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

    #[test]
    fn json_rpc_request_with_params() {
        let json = r#"{"jsonrpc":"2.0","method":"opl_getBalance","params":["abc123"],"id":2}"#;
        let req: JsonRpcRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.method, "opl_getBalance");
    }

    #[test]
    fn invalid_request_error_uses_json_rpc_standard_code() {
        let err = JsonRpcError::invalid_request("bad envelope");
        assert_eq!(err.code, -32600);
        assert_eq!(err.message, "bad envelope");
    }
}
