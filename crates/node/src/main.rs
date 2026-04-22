//! Opolys node entry point.
//!
//! Starts the full node with two optional subsystems:
//!
//! 1. **RPC server** (on by default) — serves JSON-RPC 2.0 queries on `rpc_port`.
//!    Disable with `--no-rpc`.
//! 2. **Mining loop** (off by default) — continuously attempts to mine new blocks
//!    using Autolykos PoW and applies them to chain state. Enable with `--mine`.
//!
//! On startup, the node either loads persisted state from RocksDB (resuming
//! from the last known block) or initializes from genesis (if no state exists).
//! Chain info is shared with the RPC server via an `Arc<RwLock<ChainInfo>>`
//! snapshot that is refreshed after each block is applied.
//!
//! Opolys ($OPL) is a blockchain built as decentralized digital gold with no
//! hard cap. Difficulty and rewards emerge from chain state. Fees are
//! market-driven and burned. Validators earn from block rewards only.

use clap::Parser;
use opolys_node::{Args, NodeConfig, OpolysNode, ChainState};
use opolys_rpc::RpcState;
use opolys_rpc::server::ChainInfo;

/// Convert live `ChainState` into an RPC-friendly `ChainInfo` snapshot.
fn chain_state_to_info(chain: &ChainState) -> ChainInfo {
    ChainInfo {
        height: chain.current_height,
        difficulty: chain.current_difficulty,
        total_issued: chain.total_issued,
        total_burned: chain.total_burned,
        circulating_supply: chain.circulating_supply(),
        latest_block_hash: chain.latest_block_hash.to_hex(),
        phase: format!("{:?}", chain.phase),
        block_timestamps: chain.block_timestamps.clone(),
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
    };

    tracing::info!(
        port = config.listen_port,
        rpc_port = config.rpc_port,
        data_dir = %config.data_dir,
        mining = config.mine,
        rpc = !config.no_rpc,
        "Starting Opolys node"
    );

    // Initialize the node — loads persisted state from disk or starts from genesis
    let node = OpolysNode::new(config.clone());

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
    if config.no_rpc {
        tracing::info!("RPC: disabled (run without --no-rpc to enable)");
    }

    // Build the shared ChainInfo snapshot — both the RPC server and the mining
    // loop update this after each block is applied so RPC queries stay current.
    let chain_info: std::sync::Arc<tokio::sync::RwLock<ChainInfo>> = {
        let chain = node.chain.read().await;
        std::sync::Arc::new(tokio::sync::RwLock::new(chain_state_to_info(&chain)))
    };

    // Optionally start the JSON-RPC server
    let mut rpc_handle: Option<tokio::task::JoinHandle<()>> = None;
    if !config.no_rpc && node.store.is_some() {
        let rpc_state = RpcState::new(
            chain_info.clone(),
            node.accounts.clone(),
            node.validators.clone(),
            node.mempool.clone(),
            node.store.as_ref().unwrap().clone(),
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

    // Optionally start the mining loop
    let mut mining_handle: Option<tokio::task::JoinHandle<()>> = None;
    if config.mine {
        let chain_info_clone = chain_info.clone();
        mining_handle = Some(tokio::spawn(async move {
            loop {
                match node.mine_block(10_000_000).await {
                    Some(block) => {
                        let height = block.header.height;
                        let tx_count = block.transactions.len();
                        let difficulty = block.header.difficulty;

                        match node.apply_block(&block).await {
                            Ok(hash) => {
                                tracing::info!(
                                    height,
                                    difficulty,
                                    tx_count,
                                    hash = %hash.to_hex(),
                                    "Block mined and applied"
                                );

                                // Refresh the RPC chain info snapshot so queries
                                // return up-to-date data
                                {
                                    let chain = node.chain.read().await;
                                    let mut info = chain_info_clone.write().await;
                                    *info = chain_state_to_info(&chain);
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

    // Wait for whichever tasks are running
    // Both the RPC server and mining loop run indefinitely — if both are
    // active, we wait for either to finish. If neither is active, exit.
    match (rpc_handle, mining_handle) {
        (Some(rpc), Some(mining)) => {
            tokio::select! {
                _ = rpc => tracing::info!("RPC server stopped"),
                _ = mining => tracing::info!("Mining stopped"),
            }
        }
        (Some(rpc), None) => {
            rpc.await.expect("RPC server task panicked");
        }
        (None, Some(mining)) => {
            mining.await.expect("Mining task panicked");
        }
        (None, None) => {
            tracing::warn!("Node started with --no-rpc and without --mine. Nothing to do. Exiting.");
            tracing::info!("Tip: run with --mine to enable block production, or without --no-rpc to enable the RPC server.");
        }
    }
}