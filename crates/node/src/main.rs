//! Opolys node entry point.
//!
//! Starts the full node with four concurrent subsystems:
//!
//! 1. **P2P networking** — connects to peers via libp2p gossipsub/Kademlia
//! 2. **RPC server** (on by default) — serves JSON-RPC 2.0 queries on `rpc_port`
//! 3. **Mining loop** (off by default) — continuously attempts to mine new blocks
//!    using EVO-OMAP PoW. Enable with `--mine`.
//! 4. **Block submission processor** — receives blocks from external miners via
//!    `opl_submitSolution`, validates them, applies them, and updates chain state.
//!
//! On startup, the node either loads persisted state from RocksDB (resuming
//! from the last known block) or initializes from genesis (if no state exists).
//! Chain info is shared with the RPC server via an `Arc<RwLock<ChainInfo>>`
//! snapshot that is refreshed after each block is applied.
//!
//! Opolys ($OPL) is a blockchain built as decentralized digital gold with no hard cap.
//! Difficulty and rewards emerge from chain state. Fees are market-driven and burned.
//! Validators earn from block rewards only.

use clap::Parser;
use opolys_node::{Args, NodeConfig, OpolysNode, ChainState};
use opolys_rpc::RpcState;
use opolys_rpc::server::{ChainInfo, BlockSubmission, BlockSubmissionResult};
use opolys_networking::{OpolysNetwork, NetworkConfig, SyncResponse, SyncRequest, MAX_SYNC_BLOCKS};

/// Convert live `ChainState` into an RPC-friendly `ChainInfo` snapshot.
fn chain_state_to_info(chain: &ChainState) -> ChainInfo {
    ChainInfo {
        height: chain.current_height,
        difficulty: chain.current_difficulty,
        total_issued: chain.total_issued,
        total_burned: chain.total_burned,
        circulating_supply: chain.circulating_supply(),
        latest_block_hash: chain.latest_block_hash.clone(),
        state_root: chain.state_root.clone(),
        phase: format!("{:?}", chain.phase),
        block_timestamps: chain.block_timestamps.clone(),
        suggested_fee: chain.suggested_fee,
    }
}

#[tokio::main]
async fn main() {
    // Parse CLI arguments (port, data directory, log level, --mine, --no-rpc, etc.)
    let args = Args::parse();

    // Initialize structured logging with the configured level
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(&args.log_level))
        )
        .init();

    // Construct node configuration from CLI arguments
    let config = NodeConfig {
        listen_port: args.port,
        rpc_port: args.rpc_port.unwrap_or(args.port + 1),
        data_dir: args.data_dir.unwrap_or_else(|| "./data".to_string()),
        bootstrap_peers: args.bootstrap.map(|s| vec![s]).unwrap_or_default(),
        log_level: args.log_level,
        mine: args.mine,
        no_rpc: args.no_rpc,
        validate: args.validate,
        key_file: args.key_file,
    };

    tracing::info!(
        port = config.listen_port,
        rpc_port = config.rpc_port,
        data_dir = %config.data_dir,
        mining = config.mine,
        validating = config.validate,
        rpc = !config.no_rpc,
        "Starting Opolys node"
    );

    // Start P2P networking
    let net_config = NetworkConfig {
        listen_port: config.listen_port,
        bootstrap_peers: config.bootstrap_peers.clone(),
        ..Default::default()
    };

    let network = match OpolysNetwork::new(net_config).await {
        Ok(network) => {
            tracing::info!(
                peer_id = %network.local_peer_id(),
                "P2P network started"
            );
            Some(network)
        }
        Err(e) => {
            tracing::warn!("P2P networking failed to start: {}. Running without P2P.", e);
            None
        }
    };

    run_node(config, network).await;
}

/// Main node loop — runs with or without P2P networking.
///
/// The P2P event loop (if networking is available) owns the OpolysNetwork and:
/// - Receives gossiped blocks/transactions and applies/forwards them
/// - Serves block sync requests from peers
/// - Broadcasts locally-mined blocks via a channel
/// - Processes sync responses to catch up to the chain tip
async fn run_node(config: NodeConfig, network: Option<OpolysNetwork>) {
    // Initialize the node — loads persisted state from disk or starts from genesis
    let node = std::sync::Arc::new(OpolysNode::new(config.clone()));

    // Log initial chain state
    {
        let chain = node.chain.read().await;
        tracing::info!(
            height = chain.current_height,
            difficulty = chain.current_difficulty,
            issued = chain.total_issued,
            burned = chain.total_burned,
            hash = %chain.latest_block_hash.to_hex(),
            "Chain state initialized"
        );
    }

    if !config.mine {
        tracing::info!("Mining: disabled (run with --mine to enable block production)");
    }
    if config.validate {
        tracing::info!("Validation: enabled (producing PoS blocks when validator is active)");
    }
    if config.no_rpc {
        tracing::info!("RPC: disabled (run without --no-rpc to enable)");
    }

    // Build the shared ChainInfo snapshot — both the RPC server and the mining
    // loop update this after each block is applied so RPC queries stay current.
    let chain_info: std::sync::Arc<tokio::sync::RwLock<ChainInfo>> = {
        let chain = node.chain.read().await;
        std::sync::Arc::new(tokio::sync::RwLock::new(chain_state_to_info(&chain)))
    };

    // Channel for externally-submitted blocks (from opl_submitSolution).
    let (block_sender, mut block_receiver) = tokio::sync::mpsc::channel::<BlockSubmission>(32);

    // Channel for broadcasting blocks mined locally or received via RPC.
    // The P2P event loop reads from this channel and broadcasts via gossipsub.
    let (block_broadcast_tx, mut block_broadcast_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(64);

    // Optionally start the JSON-RPC server
    let mut rpc_handle: Option<tokio::task::JoinHandle<()>> = None;
    if !config.no_rpc && node.store.is_some() {
        let rpc_state = RpcState::new(
            chain_info.clone(),
            node.accounts.clone(),
            node.validators.clone(),
            node.mempool.clone(),
            node.store.as_ref().unwrap().clone(),
            block_sender,
        );

        let rpc_port = config.rpc_port;
        rpc_handle = Some(tokio::spawn(async move {
            if let Err(e) = opolys_rpc::start_server(rpc_state, rpc_port).await {
                tracing::error!("RPC server error: {}", e);
            }
        }));
        tracing::info!(port = config.rpc_port, "RPC server starting");
    } else if config.no_rpc {
        tracing::info!("RPC: disabled (run without --no-rpc to enable)");
    } else {
        tracing::warn!("RPC: disabled — no persistence layer available. Run with a data directory to enable RPC.");
    }

    // Spawn a task that processes blocks submitted by external miners.
    let block_processor_chain_info = chain_info.clone();
    let block_processor_node = node.clone();
    let block_processor_broadcast = block_broadcast_tx.clone();
    let block_processor = tokio::spawn(async move {
        while let Some(submission) = block_receiver.recv().await {
            let height = submission.block.header.height;
            let difficulty = submission.block.header.difficulty;
            let tx_count = submission.block.transactions.len();

            match block_processor_node.apply_block(&submission.block).await {
                Ok(hash) => {
                    tracing::info!(
                        height,
                        difficulty,
                        tx_count,
                        hash = %hash.to_hex(),
                        "External block applied"
                    );

                    // Refresh the RPC chain info snapshot
                    {
                        let chain = block_processor_node.chain.read().await;
                        let mut info = block_processor_chain_info.write().await;
                        *info = chain_state_to_info(&chain);
                    }

                    // Queue block for P2P broadcast (non-blocking)
                    if let Ok(block_bytes) = borsh::to_vec(&submission.block) {
                        let _ = block_processor_broadcast.try_send(block_bytes);
                    }

                    let _ = submission.reply.send(BlockSubmissionResult {
                        block_hash: Some(hash.to_hex()),
                        error: None,
                    });
                }
                Err(e) => {
                    tracing::error!(height, error = %e, "Failed to apply external block");
                    let _ = submission.reply.send(BlockSubmissionResult {
                        block_hash: None,
                        error: Some(e),
                    });
                }
            }
        }
    });

    // Optionally start the mining loop
    let mut mining_handle: Option<tokio::task::JoinHandle<()>> = None;
    if config.mine {
        let chain_info_clone = chain_info.clone();
        let mining_node = node.clone();
        let mining_broadcast = block_broadcast_tx.clone();
        mining_handle = Some(tokio::spawn(async move {
            loop {
                match mining_node.mine_block(10_000_000).await {
                    Some(block) => {
                        let height = block.header.height;
                        let tx_count = block.transactions.len();
                        let difficulty = block.header.difficulty;

                        match mining_node.apply_block(&block).await {
                            Ok(hash) => {
                                tracing::info!(
                                    height,
                                    difficulty,
                                    tx_count,
                                    hash = %hash.to_hex(),
                                    "Block mined and applied"
                                );

                                // Refresh the RPC chain info snapshot
                                {
                                    let chain = mining_node.chain.read().await;
                                    let mut info = chain_info_clone.write().await;
                                    *info = chain_state_to_info(&chain);
                                }

                                // Queue block for P2P broadcast (non-blocking)
                                if let Ok(block_bytes) = borsh::to_vec(&block) {
                                    let _ = mining_broadcast.try_send(block_bytes);
                                }
                            }
                            Err(e) => {
                                tracing::error!(height, error = %e, "Failed to apply mined block");
                            }
                        }
                    }
                    None => {
                        // No block found within the attempt limit — continue trying
                    }
                }
            }
        }));
        tracing::info!("Mining loop active");
    }

    // Optionally start the validator block production loop
    let _validator_handle: Option<tokio::task::JoinHandle<()>> = if config.validate && node.signing_key.is_some() {
        let validating_node = node.clone();
        let validating_broadcast = block_broadcast_tx.clone();
        let validating_chain_info = chain_info.clone();
        Some(tokio::spawn(async move {
            tracing::info!(miner_id = %validating_node.miner_id.to_hex(), "Validator block production loop starting");
            loop {
                // Wait for the target block time before producing
                tokio::time::sleep(std::time::Duration::from_millis(opolys_core::BLOCK_TARGET_TIME_MS)).await;

                // Only produce if we're in PoS mode (stake coverage > 0)
                let chain = validating_node.chain.read().await;
                let phase = chain.phase.clone();
                drop(chain);

                if phase != opolys_core::ConsensusPhase::ProofOfStake {
                    continue;
                }

                match validating_node.produce_pos_block().await {
                    Some(block) => {
                        let height = block.header.height;
                        let tx_count = block.transactions.len();

                        match validating_node.apply_block(&block).await {
                            Ok(hash) => {
                                tracing::info!(
                                    height,
                                    tx_count,
                                    hash = %hash.to_hex(),
                                    "Validator block produced and applied"
                                );

                                // Refresh the RPC chain info snapshot
                                {
                                    let chain = validating_node.chain.read().await;
                                    let mut info = validating_chain_info.write().await;
                                    *info = chain_state_to_info(&chain);
                                }

                                // Queue block for P2P broadcast (non-blocking)
                                if let Ok(block_bytes) = borsh::to_vec(&block) {
                                    let _ = validating_broadcast.try_send(block_bytes);
                                }
                            }
                            Err(e) => {
                                tracing::error!(height, error = %e, "Failed to apply validator block");
                            }
                        }
                    }
                    None => {
                        // Not this validator's turn or not an active validator
                    }
                }
            }
        }))
    } else if config.validate {
        tracing::warn!("--validate requires --key-file to specify a validator key");
        None
    } else {
        None
    };

    // Drop the remaining sender so block_broadcast_rx ends when all producers are done
    drop(block_broadcast_tx);

    // P2P network event loop — owns the OpolysNetwork exclusively.
    // Handles incoming blocks, transactions, and sync requests. Also broadcasts
    // locally-mined blocks received from the block_broadcast channel.
    let network_handle: Option<tokio::task::JoinHandle<()>> = if let Some(mut net) = network {
        let net_node = node.clone();
        let net_chain_info = chain_info.clone();
        Some(tokio::spawn(async move {
            tracing::info!("P2P network event loop started");
            loop {
                // Use tokio::select! to handle both P2P events and local broadcast requests
                tokio::select! {
                    event = net.next_event() => {
                        match event {
                            Some(event) => handle_network_event(
                                event, &net_node, &net_chain_info, &net
                            ).await,
                            None => {
                                tracing::info!("P2P network event stream ended");
                                break;
                            }
                        }
                    }
                    block_data = block_broadcast_rx.recv() => {
                        match block_data {
                            Some(data) => {
                                if let Err(e) = net.broadcast_block(data).await {
                                    tracing::warn!("Failed to broadcast block: {}", e);
                                }
                            }
                            None => {
                                // All broadcast senders dropped — mining/RPC stopped
                            }
                        }
                    }
                }
            }
            tracing::info!("P2P network event loop ended");
        }))
    } else {
        None
    };

    // Wait for whichever tasks are running
    if let Some(rpc) = rpc_handle {
        tokio::select! {
            _ = rpc => tracing::info!("RPC server stopped"),
        }
    }

    // Wait for mining, block processor, and network tasks
    if let Some(mining) = mining_handle {
        tokio::select! {
            _ = mining => tracing::info!("Mining stopped"),
            _ = block_processor => tracing::info!("Block processor stopped"),
        }
    } else {
        let _ = block_processor.await;
    }

    if let Some(net_handle) = network_handle {
        let _ = net_handle.await;
    }

    tracing::info!("Node shutdown complete");
}

/// Handle an incoming P2P network event.
///
/// - **GossipBlock**: Deserialize and apply the block if it extends our chain
/// - **GossipTransaction**: Deserialize and add to the mempool
/// - **SyncRequestReceived**: Serve blocks from storage if available
/// - **SyncResponseReceived**: Apply received blocks to catch up to chain tip
/// - **PeerConnected/Disconnected**: Log for visibility
async fn handle_network_event(
    event: opolys_networking::OpolysNetworkEvent,
    node: &std::sync::Arc<OpolysNode>,
    chain_info: &std::sync::Arc<tokio::sync::RwLock<ChainInfo>>,
    net: &OpolysNetwork,
) {
    match event {
        opolys_networking::OpolysNetworkEvent::GossipBlock { data, source } => {
            tracing::info!(peer = %source, size = data.len(), "Received block via gossip");

            // Reject oversized block data (max 10 MiB)
            if data.len() > opolys_core::MAX_BLOCK_SIZE_BYTES {
                tracing::warn!(peer = %source, size = data.len(), "Rejected oversized block from peer");
                return;
            }

            match borsh::from_slice::<opolys_core::Block>(&data) {
                Ok(block) => {
                    // Skip blocks we've already applied (height <= current)
                    let current_height = node.chain.read().await.current_height;
                    if block.header.height <= current_height {
                        tracing::debug!(
                            height = block.header.height,
                            current_height,
                            "Skipping already-applied block"
                        );
                        return;
                    }

                    match node.apply_block(&block).await {
                        Ok(hash) => {
                            tracing::info!(height = block.header.height, hash = %hash.to_hex(), "P2P block applied");
                            // Re-broadcast the block to peers
                            if let Ok(block_data) = borsh::to_vec(&block) {
                                if let Err(e) = net.broadcast_block(block_data).await {
                                    tracing::debug!("Failed to re-broadcast block: {}", e);
                                }
                            }
                            // Refresh chain info
                            {
                                let chain = node.chain.read().await;
                                let mut info = chain_info.write().await;
                                *info = chain_state_to_info(&chain);
                            }
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "Failed to apply P2P block");
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to deserialize P2P block");
                }
            }
        }
        opolys_networking::OpolysNetworkEvent::GossipTransaction { data, source } => {
            tracing::debug!(peer = %source, size = data.len(), "Received transaction via gossip");

            // Reject oversized transaction data (max 100 KiB)
            if data.len() > opolys_core::TX_MAX_SIZE_BYTES {
                tracing::warn!(peer = %source, size = data.len(), "Rejected oversized transaction from peer");
                return;
            }

            match borsh::from_slice::<opolys_core::Transaction>(&data) {
                Ok(tx) => {
                    // Basic verification: check tx_id, signature type, and public_key binding
                    if let Err(e) = opolys_execution::verify_transaction(&tx) {
                        tracing::warn!(peer = %source, tx_id = %tx.tx_id.to_hex(), error = %e, "Rejected invalid transaction from peer");
                        return;
                    }

                    let tx_data_for_rebroadcast = data.clone();
                    let priority = tx.fee as f64;
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    {
                        let mut mempool = node.mempool.write().await;
                        match mempool.add_transaction(tx, priority, now) {
                            Ok(()) => {
                                tracing::debug!("Added gossiped transaction to mempool");
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "Failed to add gossiped transaction to mempool");
                                return;
                            }
                        }
                    }
                    // Re-broadcast the transaction to other peers (outside mempool lock)
                    if let Err(e) = net.broadcast_transaction(tx_data_for_rebroadcast).await {
                        tracing::debug!("Failed to re-broadcast transaction: {}", e);
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to deserialize P2P transaction");
                }
            }
        }
        opolys_networking::OpolysNetworkEvent::PeerConnected { peer_id } => {
            tracing::info!(peer = %peer_id, "Peer connected");
            // When a new peer connects, request blocks they may have that we don't.
            // We request from our current_height + 1 onwards.
            let current_height = node.chain.read().await.current_height;
            let request = SyncRequest {
                start_height: current_height + 1,
                count: MAX_SYNC_BLOCKS,
            };
            if let Err(e) = net.request_blocks(peer_id, request).await {
                tracing::debug!(peer = %peer_id, error = %e, "Failed to request sync blocks from peer");
            }
        }
        opolys_networking::OpolysNetworkEvent::PeerDisconnected { peer_id } => {
            tracing::info!(peer = %peer_id, "Peer disconnected");
        }
        opolys_networking::OpolysNetworkEvent::SyncRequestReceived { peer_id, request_id, request } => {
            tracing::info!(
                peer = %peer_id,
                start_height = request.start_height,
                count = request.count,
                "Sync request received from peer"
            );
            // Serve blocks from storage
            let mut blocks = Vec::new();
            if let Some(ref store) = node.store {
                let count = request.count.min(opolys_networking::MAX_SYNC_BLOCKS);
                for height in request.start_height..request.start_height + count {
                    match store.load_block(height) {
                        Ok(Some(block)) => {
                            if let Ok(block_bytes) = borsh::to_vec(&block) {
                                blocks.push(block_bytes);
                            }
                        }
                        Ok(None) => break, // No more blocks
                        Err(e) => {
                            tracing::warn!(height, error = %e, "Failed to load block for sync");
                            break;
                        }
                    }
                }
            }
            let from_height = request.start_height;
            let response = SyncResponse { blocks, from_height };
            if let Err(e) = net.respond_sync_request(request_id, response).await {
                tracing::warn!(peer = %peer_id, error = %e, "Failed to send sync response");
            }
        }
        opolys_networking::OpolysNetworkEvent::SyncResponseReceived { peer_id, response } => {
            tracing::info!(
                peer = %peer_id,
                blocks = response.blocks.len(),
                from_height = response.from_height,
                "Sync response received"
            );
            // Apply received blocks to catch up to the chain tip
            for block_bytes in &response.blocks {
                match borsh::from_slice::<opolys_core::Block>(block_bytes) {
                    Ok(block) => {
                        match node.apply_block(&block).await {
                            Ok(hash) => {
                                tracing::info!(
                                    height = block.header.height,
                                    hash = %hash.to_hex(),
                                    "Sync block applied"
                                );
                                // Refresh chain info after each block
                                {
                                    let chain = node.chain.read().await;
                                    let mut info = chain_info.write().await;
                                    *info = chain_state_to_info(&chain);
                                }
                            }
                            Err(e) => {
                                tracing::warn!(height = block.header.height, error = %e, "Failed to apply sync block");
                                // Stop applying blocks if one fails — chain must be contiguous
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "Failed to deserialize sync block");
                    }
                }
            }
        }
    }
}