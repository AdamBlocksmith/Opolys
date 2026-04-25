//! Full P2P network implementation using libp2p.
//!
//! `OpolysNetwork` wraps a libp2p `Swarm` and provides a high-level async
//! API for the node to:
//!
//! - **Start** listening on a P2P port
//! - **Dial** bootstrap peers for initial network discovery
//! - **Broadcast** transactions and blocks via gossipsub
//! - **Request blocks** from peers via request-response for chain sync
//! - **Process** incoming network events in a background task
//!
//! The network runs on a Tokio runtime and communicates with the node
//! via channels. The node sends commands through `NetworkCommand` and
//! receives events via `NetworkEvent`.

use crate::behaviour::{OpolysBehaviour, GOSSIP_BLOCK_TOPIC, GOSSIP_TX_TOPIC, opolys_agent_string, sync_protocol};
use crate::discovery::DiscoveryConfig;
use crate::gossip::GossipConfig;
use crate::sync::{SyncConfig, SyncRequest, SyncResponse};
use opolys_core::{DEFAULT_LISTEN_PORT, PING_INTERVAL_SECS, PING_TIMEOUT_SECS};
use libp2p::gossipsub::{MessageAuthenticity, ValidationMode};
use libp2p::kad::store::MemoryStore;
use libp2p::request_response;
use libp2p::swarm::SwarmEvent;
use libp2p::PeerId;
use libp2p::StreamProtocol;
use futures::StreamExt;
use std::error::Error;
use std::fmt;
use tokio::sync::mpsc;
use tokio::sync::oneshot;

/// Commands sent from the node to the network task.
#[derive(Debug)]
pub enum NetworkCommand {
    /// Broadcast a transaction via gossipsub.
    BroadcastTransaction {
        data: Vec<u8>,
    },

    /// Broadcast a block via gossipsub.
    BroadcastBlock {
        data: Vec<u8>,
    },

    /// Request blocks from a specific peer for chain sync.
    RequestBlocks {
        peer_id: PeerId,
        request: SyncRequest,
        response_channel: oneshot::Sender<Result<SyncResponse, NetworkError>>,
    },

    /// Dial a specific peer address.
    DialPeer {
        addr: libp2p::Multiaddr,
    },

    /// Get the list of connected peers.
    GetConnectedPeers {
        response_channel: oneshot::Sender<Vec<PeerId>>,
    },
}

/// Errors that can occur during network operations.
#[derive(Debug)]
pub enum NetworkError {
    /// Failed to dial a peer.
    DialError(String),
    /// Failed to broadcast a message.
    BroadcastError(String),
    /// A sync request timed out.
    SyncTimeout,
    /// A sync request failed.
    SyncError(String),
    /// The network task has shut down.
    ChannelClosed,
}

impl fmt::Display for NetworkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NetworkError::DialError(s) => write!(f, "Dial error: {}", s),
            NetworkError::BroadcastError(s) => write!(f, "Broadcast error: {}", s),
            NetworkError::SyncTimeout => write!(f, "Sync request timed out"),
            NetworkError::SyncError(s) => write!(f, "Sync error: {}", s),
            NetworkError::ChannelClosed => write!(f, "Network channel closed"),
        }
    }
}

impl Error for NetworkError {}

/// Configuration for the Opolys P2P network.
pub struct NetworkConfig {
    /// P2P listen port (default: 4170).
    pub listen_port: u16,
    /// Bootstrap peer addresses for initial network discovery.
    pub bootstrap_peers: Vec<String>,
    /// Gossip protocol configuration.
    pub gossip_config: GossipConfig,
    /// Discovery configuration.
    pub discovery_config: DiscoveryConfig,
    /// Sync configuration.
    pub sync_config: SyncConfig,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        NetworkConfig {
            listen_port: DEFAULT_LISTEN_PORT,
            bootstrap_peers: Vec::new(),
            gossip_config: GossipConfig::default(),
            discovery_config: DiscoveryConfig::default(),
            sync_config: SyncConfig::default(),
        }
    }
}

/// The main P2P network interface for an Opolys node.
///
/// Holds the command sender and the local peer ID. The actual swarm
/// runs in a background Tokio task. Commands are sent via `command_tx`
/// and events are received via `event_rx`.
pub struct OpolysNetwork {
    /// Sender for commands to the network task.
    command_tx: mpsc::Sender<NetworkCommand>,
    /// The local peer ID derived from the swarm's keypair.
    local_peer_id: PeerId,
    /// Receiver for events from the network task.
    event_rx: mpsc::Receiver<crate::behaviour::OpolysNetworkEvent>,
}

impl OpolysNetwork {
    /// Create and start the P2P network.
    ///
    /// Initializes a libp2p swarm with gossipsub, Kademlia, identify,
    /// ping, and request-response protocols. Listens on the configured
    /// port and dials bootstrap peers.
    pub async fn new(config: NetworkConfig) -> Result<Self, Box<dyn Error>> {
        let local_key = libp2p::identity::Keypair::generate_ed25519();
        let local_peer_id = local_key.public().to_peer_id();

        let gossipsub_config = libp2p::gossipsub::ConfigBuilder::default()
            .heartbeat_interval(std::time::Duration::from_secs(1))
            .validation_mode(ValidationMode::Strict)
            .max_transmit_size(config.gossip_config.max_message_size)
            .build()
            .map_err(|e| format!("Gossipsub config error: {}", e))?;

        let gossipsub = libp2p::gossipsub::Behaviour::new(
            MessageAuthenticity::Signed(local_key.clone()),
            gossipsub_config,
        )
        .map_err(|e| format!("Gossipsub init error: {}", e))?;

        let store = MemoryStore::new(local_peer_id);
        let kademlia_config = libp2p::kad::Config::new(
            StreamProtocol::new("/opolys/kad/1"),
        );
        let kademlia = libp2p::kad::Behaviour::with_config(local_peer_id, store, kademlia_config);

        let identify = libp2p::identify::Behaviour::new(
            libp2p::identify::Config::new(
                opolys_core::NETWORK_PROTOCOL_VERSION.to_string(),
                local_key.public(),
            )
            .with_agent_version(opolys_agent_string())
            .with_push_listen_addr_updates(true),
        );

        let ping = libp2p::ping::Behaviour::new(
            libp2p::ping::Config::new()
                .with_interval(std::time::Duration::from_secs(PING_INTERVAL_SECS))
                .with_timeout(std::time::Duration::from_secs(PING_TIMEOUT_SECS)),
        );

        let request_response = request_response::cbor::Behaviour::new(
            [(sync_protocol(), request_response::ProtocolSupport::Full)],
            request_response::Config::default(),
        );

        let behaviour = OpolysBehaviour {
            gossipsub,
            kademlia,
            identify,
            ping,
            request_response,
        };

        let swarm = libp2p::SwarmBuilder::with_existing_identity(local_key.clone())
            .with_tokio()
            .with_quic()
            .with_relay_client(libp2p::noise::Config::new, libp2p::yamux::Config::default)?
            .with_behaviour(|_, _| behaviour)?
            .with_swarm_config(|cfg| {
                cfg.with_idle_connection_timeout(std::time::Duration::from_secs(60))
            })
            .build();

        let (command_tx, command_rx) = mpsc::channel(256);
        let (event_tx, event_rx) = mpsc::channel(256);

        let network = OpolysNetwork {
            command_tx,
            local_peer_id,
            event_rx,
        };

        // Start the swarm event loop in a background task
        let network_task = SwarmTask {
            swarm,
            command_rx,
            event_tx,
        };
        tokio::spawn(network_task.run(config));

        Ok(network)
    }

    /// The local peer ID for this node.
    pub fn local_peer_id(&self) -> PeerId {
        self.local_peer_id
    }

    /// Receive the next network event (blocking).
    pub async fn next_event(&mut self) -> Option<crate::behaviour::OpolysNetworkEvent> {
        self.event_rx.recv().await
    }

    /// Broadcast a transaction via gossipsub.
    pub async fn broadcast_transaction(&self, data: Vec<u8>) -> Result<(), NetworkError> {
        self.command_tx
            .send(NetworkCommand::BroadcastTransaction { data })
            .await
            .map_err(|_| NetworkError::ChannelClosed)
    }

    /// Broadcast a block via gossipsub.
    pub async fn broadcast_block(&self, data: Vec<u8>) -> Result<(), NetworkError> {
        self.command_tx
            .send(NetworkCommand::BroadcastBlock { data })
            .await
            .map_err(|_| NetworkError::ChannelClosed)
    }

    /// Request blocks from a peer for chain synchronization.
    pub async fn request_blocks(
        &self,
        peer_id: PeerId,
        request: SyncRequest,
    ) -> Result<SyncResponse, NetworkError> {
        let (tx, rx) = oneshot::channel();
        self.command_tx
            .send(NetworkCommand::RequestBlocks {
                peer_id,
                request,
                response_channel: tx,
            })
            .await
            .map_err(|_| NetworkError::ChannelClosed)?;
        rx.await.map_err(|_| NetworkError::ChannelClosed)?
    }

    /// Get the list of currently connected peers.
    pub async fn connected_peers(&self) -> Result<Vec<PeerId>, NetworkError> {
        let (tx, rx) = oneshot::channel();
        self.command_tx
            .send(NetworkCommand::GetConnectedPeers { response_channel: tx })
            .await
            .map_err(|_| NetworkError::ChannelClosed)?;
        rx.await.map_err(|_| NetworkError::ChannelClosed)
    }
}

/// Background task that runs the libp2p swarm event loop.
struct SwarmTask {
    swarm: libp2p::Swarm<OpolysBehaviour>,
    command_rx: mpsc::Receiver<NetworkCommand>,
    event_tx: mpsc::Sender<crate::behaviour::OpolysNetworkEvent>,
}

impl SwarmTask {
    /// Main event loop: processes both swarm events and node commands.
    async fn run(mut self, config: NetworkConfig) {
        // Subscribe to gossip topics
        let tx_topic = libp2p::gossipsub::IdentTopic::new(GOSSIP_TX_TOPIC);
        let block_topic = libp2p::gossipsub::IdentTopic::new(GOSSIP_BLOCK_TOPIC);

        if let Err(e) = self.swarm.behaviour_mut().gossipsub.subscribe(&tx_topic) {
            tracing::error!("Failed to subscribe to tx topic: {}", e);
        }
        if let Err(e) = self.swarm.behaviour_mut().gossipsub.subscribe(&block_topic) {
            tracing::error!("Failed to subscribe to block topic: {}", e);
        }

        // Start listening
        let listen_addr = libp2p::Multiaddr::from(libp2p::multiaddr::Protocol::Ip4(
            std::net::Ipv4Addr::UNSPECIFIED,
        ))
        .with(libp2p::multiaddr::Protocol::Udp(config.listen_port))
        .with(libp2p::multiaddr::Protocol::QuicV1);

        match self.swarm.listen_on(listen_addr) {
            Ok(_) => tracing::info!("Listening on UDP port {}", config.listen_port),
            Err(e) => tracing::error!("Failed to listen on port {}: {}", config.listen_port, e),
        }

        // Also listen on TCP
        let tcp_addr = libp2p::Multiaddr::from(libp2p::multiaddr::Protocol::Ip4(
            std::net::Ipv4Addr::UNSPECIFIED,
        ))
        .with(libp2p::multiaddr::Protocol::Tcp(config.listen_port));
        match self.swarm.listen_on(tcp_addr) {
            Ok(_) => tracing::info!("Listening on TCP port {}", config.listen_port),
            Err(e) => tracing::warn!("Failed to listen on TCP port {}: {}", config.listen_port, e),
        }

        // Dial bootstrap peers
        for peer_addr in &config.bootstrap_peers {
            match peer_addr.parse::<libp2p::Multiaddr>() {
                Ok(addr) => {
                    if let Err(e) = self.swarm.dial(addr) {
                        tracing::warn!("Failed to dial bootstrap peer: {}", e);
                    }
                }
                Err(e) => {
                    tracing::warn!("Invalid bootstrap address '{}': {}", peer_addr, e);
                }
            }
        }

        // Main event loop
        loop {
            let event = tokio::select! {
                event = self.swarm.select_next_some() => event,
                command = self.command_rx.recv() => {
                    match command {
                        Some(cmd) => {
                            self.handle_command(cmd);
                            continue;
                        }
                        None => {
                            tracing::info!("Network command channel closed, shutting down");
                            return;
                        }
                    }
                }
            };
            self.handle_swarm_event(event);
        }
    }

    fn handle_command(&mut self, command: NetworkCommand) {
        match command {
            NetworkCommand::BroadcastTransaction { data } => {
                let topic = libp2p::gossipsub::IdentTopic::new(GOSSIP_TX_TOPIC);
                if let Err(e) = self.swarm.behaviour_mut().gossipsub.publish(topic, data) {
                    tracing::warn!("Failed to broadcast transaction: {}", e);
                }
            }
            NetworkCommand::BroadcastBlock { data } => {
                let topic = libp2p::gossipsub::IdentTopic::new(GOSSIP_BLOCK_TOPIC);
                if let Err(e) = self.swarm.behaviour_mut().gossipsub.publish(topic, data) {
                    tracing::warn!("Failed to broadcast block: {}", e);
                }
            }
            NetworkCommand::RequestBlocks { peer_id, request, response_channel: _ } => {
                self.swarm.behaviour_mut().request_response.send_request(&peer_id, request);
            }
            NetworkCommand::DialPeer { addr } => {
                if let Err(e) = self.swarm.dial(addr) {
                    tracing::warn!("Failed to dial peer: {}", e);
                }
            }
            NetworkCommand::GetConnectedPeers { response_channel } => {
                let peers: Vec<PeerId> = self.swarm.connected_peers().cloned().collect();
                let _ = response_channel.send(peers);
            }
        }
    }

    fn handle_swarm_event(
        &mut self,
        event: SwarmEvent<
            <OpolysBehaviour as libp2p::swarm::NetworkBehaviour>::ToSwarm,
        >,
    ) {
        match event {
            SwarmEvent::Behaviour(behaviour_event) => {
                // Route the composed behaviour event to the appropriate handler
                tracing::debug!("Composed behaviour event: {:?}", behaviour_event);
            }
            SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                tracing::info!("Peer connected: {}", peer_id);
                let _ = self.event_tx.try_send(
                    crate::behaviour::OpolysNetworkEvent::PeerConnected { peer_id },
                );
            }
            SwarmEvent::ConnectionClosed { peer_id, .. } => {
                tracing::info!("Peer disconnected: {}", peer_id);
                let _ = self.event_tx.try_send(
                    crate::behaviour::OpolysNetworkEvent::PeerDisconnected { peer_id },
                );
            }
            SwarmEvent::NewListenAddr { address, .. } => {
                tracing::info!("Listening on {}", address);
            }
            SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                tracing::warn!("Outgoing connection error to {:?}: {}", peer_id, error);
            }
            _ => {
                tracing::trace!("Swarm event: {:?}", event);
            }
        }
    }
}