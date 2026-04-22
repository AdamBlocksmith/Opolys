//! Opolys node entry point.
//!
//! Starts the full node, which comprises two concurrent tasks:
//!
//! 1. **RPC server** — serves JSON-RPC 2.0 queries on `rpc_port`
//! 2. **Mining loop** — continuously attempts to mine new blocks using
//!    Autolykos PoW and applies them to chain state
//!
//! On startup, the node either loads persisted state from RocksDB (resuming
//! from the last known block) or initializes from genesis (if no state exists).
//! Chain info is shared with the RPC server via an `Arc<RwLock<ChainInfo>>`
//! snapshot that is refreshed after each block.
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
    }
}

#[tokio::main]
async fn main() {
    // Parse CLI arguments (port, data directory, log level, etc.)
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
    };

    tracing::info!(port = config.listen_port, rpc_port = config.rpc_port, data_dir = %config.data_dir, "Starting Opolys node");

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

    // Build the RPC ChainInfo snapshot from the current chain state
    let chain_info = {
        let chain = node.chain.read().await;
        chain_state_to_info(&chain)
    };
    let rpc_state = RpcState::new(
        std::sync::Arc::new(tokio::sync::RwLock::new(chain_info)),
        node.accounts.clone(),
        node.validators.clone(),
    );

    // Start the JSON-RPC server on the configured port
    let rpc_port = config.rpc_port;
    let rpc_handle = tokio::spawn(async move {
        if let Err(e) = opolys_rpc::start_server(rpc_state, rpc_port).await {
            tracing::error!("RPC server error: {}", e);
        }
    });

    // Mine blocks in a tight loop — each successful mine applies the block,
    // burns transaction fees, emits the block reward, and adjusts difficulty
    let mining_handle = tokio::spawn(async move {
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
    });

    tracing::info!("Opolys node running. Mining + RPC active.");

    // Wait for either task to finish (both run indefinitely)
    tokio::select! {
        _ = rpc_handle => tracing::info!("RPC server stopped"),
        _ = mining_handle => tracing::info!("Mining stopped"),
    }
}