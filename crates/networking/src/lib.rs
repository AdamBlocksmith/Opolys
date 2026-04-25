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

pub mod gossip;
pub mod discovery;
pub mod sync;
pub mod behaviour;
pub mod network;

pub use behaviour::*;
pub use network::*;
pub use sync::{SyncRequest, SyncResponse, SyncConfig, MAX_SYNC_BLOCKS, MAX_SYNC_HEADERS};
pub use discovery::DiscoveryConfig;
pub use libp2p::request_response::InboundRequestId;