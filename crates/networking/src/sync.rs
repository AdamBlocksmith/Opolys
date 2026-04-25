//! Block synchronization protocol types and configuration.
//!
//! Opolys uses a request-response protocol for chain synchronization.
//! When a node detects it is behind, it requests blocks from peers
//! to catch up to the chain tip.

use opolys_core::{BlockHeight, SYNC_MAX_BLOCKS_PER_REQUEST, SYNC_MAX_HEADERS_PER_REQUEST};
use serde::{Deserialize, Serialize};

/// Maximum number of blocks that can be requested in a single sync request.
pub const MAX_SYNC_BLOCKS: u64 = SYNC_MAX_BLOCKS_PER_REQUEST;
/// Maximum number of headers that can be requested in a single sync request.
pub const MAX_SYNC_HEADERS: u64 = SYNC_MAX_HEADERS_PER_REQUEST;

/// Request for a range of full blocks from a peer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncRequest {
    /// The starting block height to fetch from.
    pub start_height: BlockHeight,
    /// Number of blocks to request (capped at MAX_SYNC_BLOCKS).
    pub count: u64,
}

/// Response containing blocks from a peer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncResponse {
    /// The blocks, serialized via Borsh for efficiency.
    pub blocks: Vec<Vec<u8>>,
    /// The height of the first block in the response.
    pub from_height: BlockHeight,
}

/// Request for a range of block headers (without transactions).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeaderSyncRequest {
    /// The starting block height to fetch from.
    pub start_height: BlockHeight,
    /// Number of headers to request (capped at MAX_SYNC_HEADERS).
    pub count: u64,
}

/// Configuration for the block sync protocol.
pub struct SyncConfig {
    /// Maximum number of blocks per request.
    pub max_blocks_per_request: u64,
    /// Maximum number of headers per request.
    pub max_headers_per_request: u64,
    /// Timeout for individual sync requests in seconds.
    pub request_timeout_secs: u64,
    /// Number of peers to query in parallel during chain synchronization.
    pub parallel_peer_count: usize,
}

impl Default for SyncConfig {
    fn default() -> Self {
        SyncConfig {
            max_blocks_per_request: SYNC_MAX_BLOCKS_PER_REQUEST,
            max_headers_per_request: SYNC_MAX_HEADERS_PER_REQUEST,
            request_timeout_secs: 30,
            parallel_peer_count: 3,
        }
    }
}