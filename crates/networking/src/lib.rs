//! Opolys P2P networking layer using libp2p.
//!
//! Provides full peer-to-peer connectivity for the Opolys blockchain:
//!
//! - **Gossipsub** — transaction and block propagation
//! - **Kademlia DHT** — peer discovery and routing
//! - **Identify** — protocol version exchange
//! - **Ping** — liveness checking
//! - **Request-response** — block/header synchronization
//!
//! The `OpolysNetwork` struct wraps a libp2p `Swarm` and provides a
//! high-level API for the node to broadcast transactions, announce blocks,
//! request blocks from peers, and manage connections.

pub mod behaviour;
pub mod challenge;
pub mod discovery;
pub mod gossip;
pub mod network;
pub mod sync;

pub use behaviour::*;
pub use challenge::{
    CHALLENGE_TIMEOUT_SECS, ChallengeRequest, ChallengeResponse, challenge_protocol,
};
pub use discovery::{DiscoveryConfig, MAINNET_DNS_SEEDS, resolve_dns_seeds};
pub use libp2p::Multiaddr;
pub use libp2p::PeerId;
pub use libp2p::request_response::InboundRequestId;
pub use network::*;
pub use sync::{MAX_SYNC_BLOCKS, MAX_SYNC_HEADERS, SyncConfig, SyncRequest, SyncResponse};
