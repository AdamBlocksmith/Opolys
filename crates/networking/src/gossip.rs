//! Gossip protocol configuration for the Opolys P2P network.
//!
//! Defines pub/sub topics for transaction and block propagation. Transactions
//! are gossiped under `opolys/tx/v1` and blocks under `opolys/block/v1`.
//! Message size is capped at `GOSSIP_MAX_MESSAGE_SIZE_BYTES` to prevent
//! abuse.

use opolys_core::GOSSIP_MAX_MESSAGE_SIZE_BYTES;

/// Gossip topic for transaction propagation.
pub const GOSSIP_TX_TOPIC: &str = "opolys/tx/v1";
/// Gossip topic for block propagation.
pub const GOSSIP_BLOCK_TOPIC: &str = "opolys/block/v1";

/// Configuration for the gossip protocol.
pub struct GossipConfig {
    /// Maximum message size in bytes (from `opolys_core::GOSSIP_MAX_MESSAGE_SIZE_BYTES`).
    pub max_message_size: usize,
    /// Pub/sub topic name for transaction messages.
    pub tx_topic: String,
    /// Pub/sub topic name for block messages.
    pub block_topic: String,
}

impl Default for GossipConfig {
    fn default() -> Self {
        GossipConfig {
            max_message_size: GOSSIP_MAX_MESSAGE_SIZE_BYTES,
            tx_topic: GOSSIP_TX_TOPIC.to_string(),
            block_topic: GOSSIP_BLOCK_TOPIC.to_string(),
        }
    }
}

/// Gossip protocol handler for the Opolys P2P network.
///
/// Wraps a `GossipConfig` and provides accessors for topic names and
/// message size limits. The actual network transport is implemented in
/// the `sync` and `discovery` modules.
pub struct NetworkGossip {
    config: GossipConfig,
}

impl NetworkGossip {
    /// Create a new gossip handler with the given configuration.
    pub fn new(config: GossipConfig) -> Self {
        NetworkGossip { config }
    }

    /// Maximum allowed message size in bytes.
    pub fn max_message_size(&self) -> usize {
        self.config.max_message_size
    }

    /// The transaction gossip topic name.
    pub fn tx_topic(&self) -> &str {
        &self.config.tx_topic
    }

    /// The block gossip topic name.
    pub fn block_topic(&self) -> &str {
        &self.config.block_topic
    }
}