//! Gossip protocol configuration for the Opolys P2P network.
//!
//! Defines pub/sub topics for transaction, block, and attestation propagation.
//! Transactions are gossiped under `opolys/tx/v1`, blocks under
//! `opolys/block/v1`, and refiner attestations under `opolys/attestation/v1`.
//! Message size is capped at `GOSSIP_MAX_MESSAGE_SIZE_BYTES` to prevent abuse.

use opolys_core::GOSSIP_MAX_MESSAGE_SIZE_BYTES;

/// Gossip topic for transaction propagation.
pub const GOSSIP_TX_TOPIC: &str = "opolys/tx/v1";
/// Gossip topic for block propagation.
pub const GOSSIP_BLOCK_TOPIC: &str = "opolys/block/v1";
/// Gossip topic for refiner block attestations.
pub const GOSSIP_ATTESTATION_TOPIC: &str = "opolys/attestation/v1";

/// Configuration for the gossip protocol.
pub struct GossipConfig {
    /// Maximum message size in bytes.
    pub max_message_size: usize,
    /// Pub/sub topic name for transaction messages.
    pub tx_topic: String,
    /// Pub/sub topic name for block messages.
    pub block_topic: String,
    /// Pub/sub topic name for refiner attestation messages.
    pub attestation_topic: String,
}

impl Default for GossipConfig {
    fn default() -> Self {
        GossipConfig {
            max_message_size: GOSSIP_MAX_MESSAGE_SIZE_BYTES,
            tx_topic: GOSSIP_TX_TOPIC.to_string(),
            block_topic: GOSSIP_BLOCK_TOPIC.to_string(),
            attestation_topic: GOSSIP_ATTESTATION_TOPIC.to_string(),
        }
    }
}
