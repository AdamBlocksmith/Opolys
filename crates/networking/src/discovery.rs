//! Peer discovery configuration using Kademlia DHT.
//!
//! Opolys uses Kademlia for distributed peer discovery. New nodes bootstrap
//! from known peers and then use the DHT to discover additional peers
//! on the network.

use opolys_core::KAD_BUCKET_SIZE;

/// Configuration for Kademlia DHT peer discovery.
pub struct DiscoveryConfig {
    /// Kademlia bucket size (k-parameter).
    pub bucket_size: usize,
    /// Timeout for DHT queries in seconds.
    pub query_timeout_secs: u64,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        DiscoveryConfig {
            bucket_size: KAD_BUCKET_SIZE,
            query_timeout_secs: 60,
        }
    }
}