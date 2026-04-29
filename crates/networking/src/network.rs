//! Full P2P network implementation using libp2p.
//!
//! `OpolysNetwork` wraps a libp2p `Swarm` and provides a high-level async
//! API for the node to:
//!
//! - **Start** listening on a P2P port
//! - **Dial** bootstrap peers for initial network discovery
//! - **Broadcast** transactions and blocks via gossipsub
//! - **Request blocks** from peers via request-response for chain sync
//! - **Respond** to block sync requests from other peers
//! - **Process** incoming network events in a background task
//!
//! The network runs on a Tokio runtime and communicates with the node
//! via channels. The node sends commands through `NetworkCommand` and
//! receives events via `NetworkEvent`.

use crate::behaviour::{OpolysBehaviour, GOSSIP_BLOCK_TOPIC, GOSSIP_TX_TOPIC, opolys_agent_string, sync_protocol, OpolysBehaviourEvent};
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
use std::collections::HashMap;
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
    },

    /// Respond to an inbound sync request with blocks.
    /// Takes the request_id (from SyncRequestReceived event) and the response.
    RespondSyncRequest {
        request_id: request_response::InboundRequestId,
        response: SyncResponse,
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
            .with_relay_client(libp2p::noise::Config::new, || libp2p::yamux::Config::default())?
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
            // Tracks inbound sync request response channels keyed by request_id.
            // When we receive a SyncRequestReceived event, we store the channel
            // here so the node can respond via RespondSyncRequest.
            inbound_request_channels: HashMap::new(),
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
    /// The response will be delivered as a SyncResponseReceived event.
    pub async fn request_blocks(
        &self,
        peer_id: PeerId,
        request: SyncRequest,
    ) -> Result<(), NetworkError> {
        self.command_tx
            .send(NetworkCommand::RequestBlocks { peer_id, request })
            .await
            .map_err(|_| NetworkError::ChannelClosed)
    }

    /// Respond to an inbound sync request with blocks.
    pub async fn respond_sync_request(
        &self,
        request_id: request_response::InboundRequestId,
        response: SyncResponse,
    ) -> Result<(), NetworkError> {
        self.command_tx
            .send(NetworkCommand::RespondSyncRequest { request_id, response })
            .await
            .map_err(|_| NetworkError::ChannelClosed)
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
    /// Pending inbound sync request response channels, keyed by request_id.
    /// When the node receives a SyncRequestReceived event, it can respond
    /// using NetworkCommand::RespondSyncRequest, which looks up the channel here.
    inbound_request_channels: HashMap<request_response::InboundRequestId, request_response::ResponseChannel<SyncResponse>>,
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

        // Start listening on QUIC (primary transport for P2P)
        let listen_addr = libp2p::Multiaddr::from(libp2p::multiaddr::Protocol::Ip4(
            std::net::Ipv4Addr::UNSPECIFIED,
        ))
        .with(libp2p::multiaddr::Protocol::Udp(config.listen_port))
        .with(libp2p::multiaddr::Protocol::QuicV1);

        match self.swarm.listen_on(listen_addr) {
            Ok(_) => tracing::info!("Listening on UDP/QUIC port {}", config.listen_port),
            Err(e) => tracing::error!("Failed to listen on port {}: {}", config.listen_port, e),
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
            NetworkCommand::RequestBlocks { peer_id, request } => {
                let request_id = self.swarm.behaviour_mut().request_response.send_request(&peer_id, request);
                tracing::debug!(%peer_id, ?request_id, "Sent sync request to peer");
            }
            NetworkCommand::RespondSyncRequest { request_id, response } => {
                if let Some(channel) = self.inbound_request_channels.remove(&request_id) {
                    if let Err(e) = self.swarm.behaviour_mut().request_response.send_response(channel, response) {
                        tracing::warn!(?request_id, "Failed to send sync response: {:?}", e);
                    }
                } else {
                    tracing::warn!(?request_id, "No pending inbound request channel for sync response");
                }
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
            SwarmEvent::Behaviour(composed_event) => {
                self.handle_composed_event(composed_event);
            }
            SwarmEvent::ConnectionEstablished { peer_id, endpoint, .. } => {
                tracing::info!("Peer connected: {}", peer_id);
                // Only outbound connections have a reliable dialable address to cache.
                let addr = match &endpoint {
                    libp2p::core::ConnectedPoint::Dialer { address, .. } => Some(address.clone()),
                    libp2p::core::ConnectedPoint::Listener { .. } => None,
                };
                let _ = self.event_tx.try_send(
                    crate::behaviour::OpolysNetworkEvent::PeerConnected { peer_id, addr },
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
                tracing::trace!("Unhandled swarm event: {:?}", event);
            }
        }
    }

    /// Route composed behaviour events to the appropriate handler.
    ///
    /// The `#[derive(NetworkBehaviour)]` macro generates `OpolysBehaviourEvent`
    /// with variants matching the struct field names (in UpperCamelCase):
    /// Gossipsub, Kademlia, Identify, Ping, RequestResponse.
    fn handle_composed_event(
        &mut self,
        event: OpolysBehaviourEvent,
    ) {
        match event {
            // ── Gossipsub events ─────────────────────────────────────────
            // Route incoming gossip messages to the node based on their topic.
            // Transactions arrive on the tx topic; blocks on the block topic.
            OpolysBehaviourEvent::Gossipsub(gossip_event) => {
                match gossip_event {
                    libp2p::gossipsub::Event::Message {
                        propagation_source,
                        message,
                        ..
                    } => {
                        let topic = message.topic.to_string();
                        let data = message.data;
                        let source = propagation_source;

                        if topic == GOSSIP_TX_TOPIC {
                            let _ = self.event_tx.try_send(
                                crate::behaviour::OpolysNetworkEvent::GossipTransaction {
                                    data,
                                    source,
                                },
                            );
                        } else if topic == GOSSIP_BLOCK_TOPIC {
                            let _ = self.event_tx.try_send(
                                crate::behaviour::OpolysNetworkEvent::GossipBlock {
                                    data,
                                    source,
                                },
                            );
                        } else {
                            tracing::warn!(topic = %topic, "Unknown gossip topic");
                        }
                    }
                    libp2p::gossipsub::Event::Subscribed { peer_id, topic } => {
                        tracing::debug!(peer = %peer_id, topic = %topic, "Peer subscribed to topic");
                    }
                    libp2p::gossipsub::Event::Unsubscribed { peer_id, topic } => {
                        tracing::debug!(peer = %peer_id, topic = %topic, "Peer unsubscribed from topic");
                    }
                    libp2p::gossipsub::Event::GossipsubNotSupported { peer_id } => {
                        tracing::debug!(peer = %peer_id, "Peer does not support gossipsub");
                    }
                }
            }

            // ── Request-Response events ────────────────────────────────────
            // Sync requests arrive as inbound Request messages; sync responses
            // arrive as inbound Response messages for our outbound requests.
            OpolysBehaviourEvent::RequestResponse(rr_event) => {
                match rr_event {
                    libp2p::request_response::Event::Message { peer, message } => {
                        match message {
                            // Inbound sync request from a peer — save the response
                            // channel and emit an event so the node can look up blocks
                            // and respond asynchronously.
                            libp2p::request_response::Message::Request { request_id, request, channel } => {
                                tracing::info!(peer = %peer, ?request_id, start_height = request.start_height, count = request.count, "Sync request received");
                                self.inbound_request_channels.insert(request_id, channel);
                                let _ = self.event_tx.try_send(
                                    crate::behaviour::OpolysNetworkEvent::SyncRequestReceived {
                                        peer_id: peer,
                                        request_id,
                                        request,
                                    },
                                );
                            }
                            // Inbound response to our outbound sync request — forward
                            // to the node as a SyncResponseReceived event.
                            libp2p::request_response::Message::Response { request_id, response } => {
                                tracing::debug!(peer = %peer, ?request_id, blocks = response.blocks.len(), "Sync response received");
                                let _ = self.event_tx.try_send(
                                    crate::behaviour::OpolysNetworkEvent::SyncResponseReceived {
                                        peer_id: peer,
                                        response,
                                    },
                                );
                            }
                        }
                    }
                    libp2p::request_response::Event::OutboundFailure { peer, request_id, error } => {
                        tracing::warn!(peer = %peer, ?request_id, ?error, "Outbound sync request failed");
                    }
                    libp2p::request_response::Event::InboundFailure { peer, request_id, error } => {
                        tracing::warn!(peer = %peer, ?request_id, ?error, "Inbound sync request failed");
                    }
                    libp2p::request_response::Event::ResponseSent { peer, request_id } => {
                        tracing::debug!(peer = %peer, ?request_id, "Sync response sent");
                    }
                }
            }

            // ── Identify events ────────────────────────────────────────────
            OpolysBehaviourEvent::Identify(identify_event) => {
                match identify_event {
                    libp2p::identify::Event::Received { peer_id, info, .. } => {
                        tracing::debug!(peer = %peer_id, agent = %info.agent_version, "Identify received");
                        // Add peer addresses to Kademlia DHT for better routing
                        for addr in info.listen_addrs {
                            self.swarm.behaviour_mut().kademlia.add_address(&peer_id, addr);
                        }
                    }
                    libp2p::identify::Event::Sent { peer_id, .. } => {
                        tracing::trace!(peer = %peer_id, "Identify sent");
                    }
                    libp2p::identify::Event::Pushed { peer_id, .. } => {
                        tracing::trace!(peer = %peer_id, "Identify pushed");
                    }
                    libp2p::identify::Event::Error { peer_id, error, .. } => {
                        tracing::debug!(peer = %peer_id, ?error, "Identify error");
                    }
                }
            }

            // ── Kademlia events ───────────────────────────────────────────
            OpolysBehaviourEvent::Kademlia(kad_event) => {
                tracing::trace!("Kademlia event: {:?}", kad_event);
            }

            // ── Ping events ────────────────────────────────────────────────
            // The ping event is a struct (not an enum) in libp2p 0.54.
            OpolysBehaviourEvent::Ping(ping_event) => {
                match ping_event.result {
                    Ok(rtt) => {
                        tracing::trace!(peer = %ping_event.peer, rtt_ms = rtt.as_millis(), "Ping succeeded");
                    }
                    Err(error) => {
                        tracing::debug!(peer = %ping_event.peer, ?error, "Ping failed");
                    }
                }
            }
        }
    }
}