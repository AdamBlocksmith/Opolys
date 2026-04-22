use clap::Parser;
use opolys_node::{Args, NodeConfig, OpolysNode};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    let args = Args::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new(&args.log_level))
        )
        .init();

    let config = NodeConfig {
        listen_port: args.port,
        rpc_port: args.rpc_port.unwrap_or(args.port + 1),
        data_dir: args.data_dir.unwrap_or_else(|| "./data".to_string()),
        bootstrap_peers: args.bootstrap.map(|s| vec![s]).unwrap_or_default(),
        log_level: args.log_level,
    };

    tracing::info!("Starting Opolys node on port {}", config.listen_port);
    tracing::info!("RPC port: {}", config.rpc_port);
    tracing::info!("Data directory: {}", config.data_dir);

    let node = OpolysNode::new(config);

    tracing::info!("Opolys node initialized. Chain height: {}", 
        node.chain.blocking_read().current_height);
    tracing::info!("Waiting for connections...");

    loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    }
}