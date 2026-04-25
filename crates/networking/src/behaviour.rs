//! Combined libp2p network behaviour for Opolys.
//!
//! Composes gossipsub (transaction/block propagation), Kademlia DHT
//! (peer discovery), identify (protocol handshake), ping (liveness),
//! and request-response (block sync) into a single `NetworkBehaviour`.

use crate::sync::{SyncRequest, SyncResponse};
use libp2p::kad::store::MemoryStore;
use libp2p::request_response;
use libp2p::swarm::NetworkBehaviour;
use libp2p::StreamProtocol;
use opolys_core::NETWORK_PROTOCOL_VERSION;

/// Gossip topic for transaction propagation.
pub const GOSSIP_TX_TOPIC: &str = "opolys/tx/v1";
/// Gossip topic for block propagation.
pub const GOSSIP_BLOCK_TOPIC: &str = "opolys/block/v1";

/// The sync protocol name for request-response.
pub const SYNC_PROTOCOL_NAME: &str = "/opolys/sync/1";

/// The sync protocol for block/header sync via request-response.
/// Uses CBOR serialization for flexible message encoding.
pub fn sync_protocol() -> StreamProtocol {
    StreamProtocol::new(SYNC_PROTOCOL_NAME)
}

/// The combined network behaviour for an Opolys node.
///
/// This composition lets the swarm handle all five protocols
/// simultaneously: gossipsub for block/tx broadcast, Kademlia for
/// DHT-based peer discovery, identify for protocol versioning,
/// ping for liveness, and request-response for block sync.
#[derive(NetworkBehaviour)]
pub struct OpolysBehaviour {
    /// Gossipsub for broadcasting transactions and blocks.
    pub gossipsub: libp2p::gossipsub::Behaviour,

    /// Kademlia DHT for peer discovery and routing.
    pub kademlia: libp2p::kad::Behaviour<MemoryStore>,

    /// Identify protocol for exchanging agent strings and addresses.
    pub identify: libp2p::identify::Behaviour,

    /// Ping for connection liveness checks.
    pub ping: libp2p::ping::Behaviour,

    /// Request-response protocol for block/header sync.
    pub request_response: request_response::cbor::Behaviour<SyncRequest, SyncResponse>,
}

/// Events emitted by the network behaviour, consumed by the node.
#[derive(Debug)]
pub enum OpolysNetworkEvent {
    /// A transaction was received via gossipsub.
    GossipTransaction {
        /// Borsh-serialized transaction bytes.
        data: Vec<u8>,
        /// The peer that sent this transaction.
        source: libp2p::PeerId,
    },

    /// A block was received via gossipsub.
    GossipBlock {
        /// Borsh-serialized block bytes.
        data: Vec<u8>,
        /// The peer that sent this block.
        source: libp2p::PeerId,
    },

    /// A peer connected to us.
    PeerConnected {
        peer_id: libp2p::PeerId,
    },

    /// A peer disconnected from us.
    PeerDisconnected {
        peer_id: libp2p::PeerId,
    },

    /// A sync response was received from a peer.
    SyncResponseReceived {
        peer_id: libp2p::PeerId,
        response: SyncResponse,
    },

    /// A sync request was received from a peer.
    SyncRequestReceived {
        peer_id: libp2p::PeerId,
        request: SyncRequest,
    },
}

/// Construct the Opolys agent string for the identify protocol.
pub fn opolys_agent_string() -> String {
    format!("/opolys/{}", NETWORK_PROTOCOL_VERSION)
}