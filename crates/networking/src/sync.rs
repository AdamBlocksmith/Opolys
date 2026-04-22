use opolys_core::{Block, BlockHeight, SYNC_MAX_BLOCKS_PER_REQUEST, SYNC_MAX_HEADERS_PER_REQUEST};

#[derive(Debug, Clone)]
pub struct SyncRequest {
    pub start_height: BlockHeight,
    pub count: u64,
    pub include_transactions: bool,
}

#[derive(Debug, Clone)]
pub struct SyncResponse {
    pub blocks: Vec<Block>,
    pub from_height: BlockHeight,
}

#[derive(Debug, Clone)]
pub struct HeaderSyncRequest {
    pub start_height: BlockHeight,
    pub count: u64,
}

pub struct SyncConfig {
    pub max_blocks_per_request: u64,
    pub max_headers_per_request: u64,
    pub request_timeout_secs: u64,
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