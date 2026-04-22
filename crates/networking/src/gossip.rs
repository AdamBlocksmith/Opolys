use opolys_core::{Block, Transaction, DEFAULT_LISTEN_PORT, GOSSIP_MAX_MESSAGE_SIZE_BYTES};
use std::sync::Arc;
use tokio::sync::RwLock;

pub const GOSSIP_TX_TOPIC: &str = "opolys/tx/v1";
pub const GOSSIP_BLOCK_TOPIC: &str = "opolys/block/v1";

pub struct GossipConfig {
    pub max_message_size: usize,
    pub tx_topic: String,
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

pub struct NetworkGossip {
    config: GossipConfig,
}

impl NetworkGossip {
    pub fn new(config: GossipConfig) -> Self {
        NetworkGossip { config }
    }

    pub fn max_message_size(&self) -> usize {
        self.config.max_message_size
    }

    pub fn tx_topic(&self) -> &str {
        &self.config.tx_topic
    }

    pub fn block_topic(&self) -> &str {
        &self.config.block_topic
    }
}